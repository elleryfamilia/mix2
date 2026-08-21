//! OS-level write confinement for a coordinator (lead) process.
//!
//! The product guarantee is that the lead only writes `.mix2/` in the cwd
//! (plus a narrow session area). Harnesses whose own CLIs can enforce that
//! do; for the others we wrap the lead process in an OS sandbox so *any*
//! harness can coordinate with the write scope enforced by the kernel, not
//! by the model's cooperation.
//!
//! This module is pure policy + command generation — it never spawns. The
//! runner wraps a lead command with [`wrap`] just before spawning; the
//! generated argv execs the real command under the platform engine.
//!
//! ## What is (and isn't) enforced
//!
//! The sandbox scopes **filesystem writes**. It deliberately does **not**
//! filter network egress (the lead must reach its provider API), and reads
//! are open except for an explicit credential deny-list. A prompt-injected
//! lead can still read project files (including `.env`) and exfiltrate over
//! the network — the same exposure the native `--allowedTools` lead has
//! today. The picker discloses this; callers must not imply full
//! confinement.
//!
//! ## macOS (this module)
//!
//! `sandbox-exec` with an inline (`-p`) Seatbelt profile. Paths ride in as
//! `-D KEY=value` parameters so nothing user-controlled is interpolated
//! into the profile text. `sandbox-exec` execs the target in place (same
//! PID), so the runner's process-group kill strategy is unaffected. Linux
//! (`bwrap`) lands in a follow-up.

use std::path::{Path, PathBuf};

/// Which OS mechanism backs the sandbox. Linux (`Bwrap`) is added in a
/// follow-up; the enum exists now so the policy/wrap seam is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxEngine {
    /// macOS `sandbox-exec` (Seatbelt).
    Seatbelt,
}

/// Absolute path to the macOS sandbox binary — the same trusted path the
/// availability probe validates, never a `PATH` lookup.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// A resolved, kernel-enforceable write policy for one lead process.
///
/// All paths are expected to be **canonical and non-symlink** — build the
/// writable roots through [`prepare_writable_root`], which enforces that.
/// Canonicalization matters: on macOS `/tmp` is a symlink to `/private/tmp`,
/// and an uncanonicalized `-D` param makes the allowlist fail *closed*
/// (the intended write is denied), so a lead that couldn't write `.mix2/`
/// would break visibly rather than escape.
#[derive(Debug, Clone, Default)]
pub struct SandboxPolicy {
    /// Directories the process may write (subpath grants).
    pub writable: Vec<PathBuf>,
    /// Paths denied write even inside a granted `writable` parent — the
    /// execute-and-instruction surfaces (a harness's `hooks.json`,
    /// `mcp.json`, `skills/`, …) whose next-run contents would run outside
    /// the sandbox. Subpath semantics.
    pub deny_write: Vec<PathBuf>,
    /// Directories denied both read and write (credential stores, and other
    /// harnesses' auth files). Subpath semantics; wins over every allow.
    pub deny_read_write: Vec<PathBuf>,
    /// Whether network egress is allowed. Always `true` in v1 — the
    /// guarantee is write-scoping. `false` emits a `(deny network*)` clause.
    pub allow_network: bool,
}

impl SandboxPolicy {
    /// A policy that grants exactly the given writable roots, with network
    /// open and no denies. Callers layer credential/exec-surface denies on
    /// top via the field setters.
    pub fn with_writable(writable: Vec<PathBuf>) -> Self {
        Self {
            writable,
            deny_write: Vec::new(),
            deny_read_write: Vec::new(),
            allow_network: true,
        }
    }
}

/// Prepare a directory to be a sandbox writable root: create it (parents
/// too) when `create` is set — the lead's `.mix2/` may not exist yet —
/// then reject it if the final component is a symlink (a symlinked root
/// would grant writes to its target, outside the intended tree) and return
/// its canonical path.
///
/// The symlink check is done on the final component *before* canonicalizing
/// (which would silently follow it); intermediate symlinks are collapsed by
/// canonicalization, and containment of the result is the caller's to
/// assert when it matters.
pub fn prepare_writable_root(path: &Path, create: bool) -> std::io::Result<PathBuf> {
    if create {
        std::fs::create_dir_all(path)?;
    }
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "sandbox writable root {} is a symlink; refusing to grant writes through it",
                path.display()
            ),
        ));
    }
    if !meta.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "sandbox writable root {} is not a directory",
                path.display()
            ),
        ));
    }
    path.canonicalize()
}

/// A parameterized Seatbelt profile: the SBPL text plus the `KEY=value`
/// parameter bindings it references. Paths live only in `params`, never in
/// `profile`, so no path can alter the profile's structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatbeltProfile {
    pub profile: String,
    pub params: Vec<(String, String)>,
}

/// Generate the Seatbelt profile for a policy.
///
/// Shape (SBPL is last-match-wins, so order is load-bearing):
/// 1. `(allow default)` — read/exec/network open,
/// 2. `(deny file-write*)` — then take all writes away,
/// 3. re-allow writes under each `writable` subpath,
/// 4. deny writes to each `deny_write` subpath (wins over step 3 — exec
///    surfaces stay read-only even inside a writable parent),
/// 5. deny read+write to each `deny_read_write` subpath (wins over all),
/// 6. deny Apple Events and the app-scripting mach services that
///    `(allow default)` would otherwise leave open (a naive allow-default
///    profile is an `osascript`-shaped code-execution hole),
/// 7. optionally `(deny network*)`.
pub fn seatbelt_profile(policy: &SandboxPolicy) -> SeatbeltProfile {
    let mut profile = String::from("(version 1)\n(allow default)\n");
    let mut params: Vec<(String, String)> = Vec::new();

    let push_subpaths = |profile: &mut String,
                         params: &mut Vec<(String, String)>,
                         header: &str,
                         prefix: &str,
                         paths: &[PathBuf]| {
        if paths.is_empty() {
            return;
        }
        profile.push_str(header);
        for (i, path) in paths.iter().enumerate() {
            let key = format!("{prefix}{i}");
            profile.push_str(&format!("  (subpath (param \"{key}\"))\n"));
            params.push((key, path.to_string_lossy().into_owned()));
        }
        profile.push_str(")\n");
    };

    // 2 + 3: take writes away, then re-grant the writable roots.
    profile.push_str("(deny file-write*)\n");
    push_subpaths(
        &mut profile,
        &mut params,
        "(allow file-write*\n",
        "WRITE",
        &policy.writable,
    );
    // 4: exec/instruction surfaces stay unwritable even inside a grant.
    push_subpaths(
        &mut profile,
        &mut params,
        "(deny file-write*\n",
        "DENYW",
        &policy.deny_write,
    );
    // 5: credential stores — no read, no write, beats every allow.
    push_subpaths(
        &mut profile,
        &mut params,
        "(deny file-read* file-write*\n",
        "DENYRW",
        &policy.deny_read_write,
    );
    // 6: close the Apple Events / app-scripting escape allow-default leaves.
    profile.push_str(
        "(deny appleevent-send)\n\
         (deny mach-lookup\n\
         \x20 (global-name \"com.apple.appleevents\")\n\
         \x20 (global-name \"com.apple.coreservices.appleevents\"))\n",
    );
    // 7: network stays open unless explicitly withdrawn.
    if !policy.allow_network {
        profile.push_str("(deny network*)\n");
    }

    SeatbeltProfile { profile, params }
}

/// Wrap a lead command so it runs under the sandbox engine. Returns the new
/// `(program, args)` to spawn; the original command is exec'd in place by
/// the engine. On macOS this is
/// `sandbox-exec -p <profile> -D KEY=value … -- <program> <args…>`.
pub fn wrap(
    engine: SandboxEngine,
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
) -> (String, Vec<String>) {
    match engine {
        SandboxEngine::Seatbelt => {
            let SeatbeltProfile { profile, params } = seatbelt_profile(policy);
            let mut out: Vec<String> = Vec::with_capacity(args.len() + params.len() * 2 + 4);
            out.push("-p".into());
            out.push(profile);
            for (key, value) in params {
                out.push("-D".into());
                out.push(format!("{key}={value}"));
            }
            out.push("--".into());
            out.push(program.to_owned());
            out.extend(args.iter().cloned());
            (SANDBOX_EXEC.to_owned(), out)
        }
    }
}

/// Whether the macOS sandbox engine is usable here: run a trivial
/// allow-default profile around `/usr/bin/true` and require success. A
/// `which`-style existence check is insufficient — the binary can be
/// present but the mechanism unusable — so this actually exercises it.
/// Cheap, quota-free, no model involved; the discovery layer caches it.
#[cfg(target_os = "macos")]
pub fn seatbelt_available() -> bool {
    std::process::Command::new(SANDBOX_EXEC)
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
pub fn seatbelt_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> SandboxPolicy {
        SandboxPolicy {
            writable: vec![PathBuf::from("/private/tmp/proj/.mix2")],
            deny_write: vec![PathBuf::from("/home/u/.cursor/hooks.json")],
            deny_read_write: vec![PathBuf::from("/home/u/.ssh")],
            allow_network: true,
        }
    }

    #[test]
    fn profile_keeps_paths_out_of_the_text() {
        let SeatbeltProfile { profile, params } = seatbelt_profile(&policy());
        // No raw path appears in the profile body — only param references.
        assert!(!profile.contains("/private/tmp/proj/.mix2"));
        assert!(!profile.contains("/home/u/.ssh"));
        assert!(profile.contains("(subpath (param \"WRITE0\"))"));
        assert!(profile.contains("(subpath (param \"DENYW0\"))"));
        assert!(profile.contains("(subpath (param \"DENYRW0\"))"));
        assert_eq!(
            params,
            vec![
                ("WRITE0".to_owned(), "/private/tmp/proj/.mix2".to_owned()),
                ("DENYW0".to_owned(), "/home/u/.cursor/hooks.json".to_owned()),
                ("DENYRW0".to_owned(), "/home/u/.ssh".to_owned()),
            ]
        );
    }

    #[test]
    fn profile_orders_clauses_allow_then_deny_then_hardening() {
        let profile = seatbelt_profile(&policy()).profile;
        let allow_default = profile.find("(allow default)").unwrap();
        let deny_all_writes = profile.find("(deny file-write*)\n").unwrap();
        let allow_writes = profile.find("(allow file-write*\n").unwrap();
        let deny_exec = profile.find("(deny file-write*\n").unwrap();
        let deny_creds = profile.find("(deny file-read* file-write*\n").unwrap();
        let deny_appleevents = profile.find("(deny appleevent-send)").unwrap();
        // Last match wins, so denies must come after the re-grant, and the
        // credential/exec denies after the write grant.
        assert!(allow_default < deny_all_writes);
        assert!(deny_all_writes < allow_writes);
        assert!(allow_writes < deny_exec);
        assert!(deny_exec < deny_creds);
        assert!(deny_creds < deny_appleevents);
    }

    #[test]
    fn empty_sections_are_omitted_not_emitted_as_invalid_sbpl() {
        // An empty `(allow file-write*)` block is a syntax error; a policy
        // with no writable roots must simply skip it.
        let profile = seatbelt_profile(&SandboxPolicy::with_writable(vec![])).profile;
        assert!(!profile.contains("(allow file-write*\n"));
        assert!(!profile.contains("(deny file-read* file-write*\n"));
        // The base structure and hardening always survive.
        assert!(profile.contains("(deny file-write*)\n"));
        assert!(profile.contains("(deny appleevent-send)"));
    }

    #[test]
    fn network_denied_only_when_withdrawn() {
        let mut p = policy();
        assert!(!seatbelt_profile(&p).profile.contains("(deny network*)"));
        p.allow_network = false;
        assert!(seatbelt_profile(&p).profile.contains("(deny network*)"));
    }

    #[test]
    fn wrap_builds_sandbox_exec_argv() {
        let (program, args) = wrap(
            SandboxEngine::Seatbelt,
            &policy(),
            "/usr/bin/cursor-agent",
            &["--print".to_owned(), "hello world".to_owned()],
        );
        assert_eq!(program, SANDBOX_EXEC);
        assert_eq!(args[0], "-p");
        // -D bindings precede the `--` separator; the real command and its
        // args follow verbatim (never through a shell).
        let sep = args.iter().position(|a| a == "--").unwrap();
        assert!(args[1..sep].iter().any(|a| a.starts_with("WRITE0=")));
        assert_eq!(args[sep + 1], "/usr/bin/cursor-agent");
        assert_eq!(args[sep + 2], "--print");
        assert_eq!(args[sep + 3], "hello world");
    }

    #[test]
    fn prepare_writable_root_creates_and_canonicalizes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nested/.mix2");
        let prepared = prepare_writable_root(&root, true).unwrap();
        assert!(prepared.is_absolute());
        assert!(prepared.exists());
        // Canonical: equals the canonicalized real path.
        assert_eq!(prepared, root.canonicalize().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn prepare_writable_root_rejects_a_symlinked_root() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = prepare_writable_root(&link, false).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    /// The load-bearing security test: actually run `sandbox-exec` with a
    /// generated profile and assert the kernel enforces the policy. This is
    /// what proves confinement — the pure-generation tests only prove the
    /// text is well-formed. Required on macOS runners; a machine genuinely
    /// lacking the engine (rare) skips rather than false-passes.
    #[cfg(target_os = "macos")]
    #[test]
    fn behavioral_denial_matrix_enforces_the_policy() {
        use std::process::Stdio;
        if !seatbelt_available() {
            eprintln!("skipping: sandbox-exec unavailable on this host");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().canonicalize().unwrap();
        let mix2 = prepare_writable_root(&proj.join(".mix2"), true).unwrap();
        let cred = proj.join("secret");
        std::fs::create_dir(&cred).unwrap();
        std::fs::write(cred.join("token"), "s3cr3t").unwrap();
        let exec_surface = mix2.join("hooks.json");
        let readme = proj.join("README");
        std::fs::write(&readme, "hi").unwrap();

        let policy = SandboxPolicy {
            writable: vec![mix2.clone()],
            deny_write: vec![exec_surface.clone()],
            deny_read_write: vec![cred.clone()],
            allow_network: true,
        };
        let run = |script: String| -> bool {
            let (program, args) = wrap(
                SandboxEngine::Seatbelt,
                &policy,
                "/bin/sh",
                &["-c".to_owned(), script],
            );
            std::process::Command::new(program)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        };

        // Allowed: write inside .mix2, read a normal project file.
        assert!(
            run(format!("echo ok > {}/note.txt", mix2.display())),
            "write inside .mix2 must be allowed"
        );
        assert!(
            run(format!("cat {}", readme.display())),
            "reading a non-credential file must be allowed"
        );
        // Denied: write outside .mix2, write the exec surface, read creds.
        assert!(
            !run(format!("echo bad > {}/escape.txt", proj.display())),
            "write to the project root must be denied"
        );
        assert!(
            !run(format!("echo x > {}", exec_surface.display())),
            "write to the exec surface inside .mix2 must be denied"
        );
        assert!(
            !run(format!("cat {}/token", cred.display())),
            "reading the credential store must be denied"
        );
        // Denied: the symlink escape — a link inside .mix2 pointing out must
        // not let a write reach the target (Seatbelt checks the resolved
        // path). This is the attack the reviews demanded coverage for.
        let escape_target = proj.join("escape-via-link.txt");
        std::os::unix::fs::symlink(&escape_target, mix2.join("link")).unwrap();
        assert!(
            !run(format!("echo pwned > {}/link", mix2.display())),
            "writing through a symlink out of .mix2 must be denied"
        );
        assert!(
            !escape_target.exists(),
            "the symlink-escape target must never be created"
        );
    }
}
