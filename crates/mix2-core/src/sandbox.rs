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
//! ## Engines
//!
//! **macOS** — `sandbox-exec` with an inline (`-p`) Seatbelt profile. Paths
//! ride in as `-D KEY=value` parameters so nothing user-controlled is
//! interpolated into the profile text. `sandbox-exec` execs the target in
//! place (same PID), so the runner's process-group kill strategy is
//! unaffected.
//!
//! **Linux** — `bubblewrap` (`bwrap`): the whole filesystem is bound
//! read-only, the writable roots re-bound read-write on top, credential
//! dirs/files masked with `tmpfs`/`/dev/null`, and the network namespace
//! left shared (network stays open). Requires unprivileged user namespaces,
//! which some distros restrict — [`bwrap_available`] probes a real
//! invocation, not just the binary's presence.

use std::path::{Path, PathBuf};

/// Which OS mechanism backs the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxEngine {
    /// macOS `sandbox-exec` (Seatbelt).
    Seatbelt,
    /// Linux `bubblewrap` (`bwrap`).
    Bwrap,
}

/// A fully-resolved sandbox to apply to one lead invocation: the engine and
/// the write policy. Attached to an [`crate::agents::agent::AgentRequest`];
/// the runner wraps the command with it just before spawning. Absent means
/// "run natively" (teammates, and leads whose harness enforces its own
/// scoping) — its absence keeps the command byte-identical to today.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub engine: SandboxEngine,
    pub policy: SandboxPolicy,
    /// Environment variables to remove from the child before spawning
    /// (credential vars the lead has no need for). Best-effort: filesystem
    /// reads of credential *files* are denied by the policy, but inherited
    /// env is a separate channel, closed here.
    pub env_remove: Vec<String>,
}

impl SandboxSpec {
    /// Detect the platform engine, returning `None` when no usable engine is
    /// present — callers then leave the harness lead-ineligible rather than
    /// pretending to confine it. macOS → Seatbelt; Linux → bubblewrap (only
    /// if a real sandboxed invocation succeeds, since unprivileged user
    /// namespaces are restricted on some distros).
    pub fn detect_engine() -> Option<SandboxEngine> {
        if seatbelt_available() {
            return Some(SandboxEngine::Seatbelt);
        }
        if bwrap_available() {
            return Some(SandboxEngine::Bwrap);
        }
        None
    }
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
        SandboxEngine::Bwrap => {
            let out = bwrap_args(policy, program, args);
            (bwrap_program(), out)
        }
    }
}

/// The `bwrap` binary path — the first of the common install locations that
/// exists, else the bare name (resolved via `PATH`). Resolved rather than
/// bare so the same trusted path the probe validated is the one exec'd.
pub fn bwrap_program() -> String {
    for candidate in ["/usr/bin/bwrap", "/bin/bwrap", "/usr/local/bin/bwrap"] {
        if Path::new(candidate).exists() {
            return candidate.to_owned();
        }
    }
    "bwrap".to_owned()
}

/// Build the `bwrap` argv enforcing `policy`. The whole filesystem is bound
/// read-only (`--ro-bind / /`) so reads stay open and writes are denied by
/// default — including `/tmp`, which must stay read-only so the consult
/// socket under `/tmp/mix2/<session>` remains reachable and a write there
/// isn't an escape hatch. The writable roots (`.mix2`, the lead-tmp scratch
/// the child's `TMPDIR` points at, the harness state dirs) are re-bound
/// read-write on top; credential dirs are masked with an empty `tmpfs` and
/// credential files shadowed by `/dev/null`; exec-surface files are re-bound
/// read-only. The network namespace is left shared (network stays available
/// — the guarantee is write scoping), and `--new-session` isolates the
/// controlling terminal.
///
/// Order matters (later mounts win): the base `--ro-bind / /` comes first,
/// then the writable re-binds, then the denies.
fn bwrap_args(policy: &SandboxPolicy, program: &str, args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![
        "--die-with-parent".into(),
        "--new-session".into(),
        "--ro-bind".into(),
        "/".into(),
        "/".into(),
        "--dev".into(),
        "/dev".into(),
        "--proc".into(),
        "/proc".into(),
    ];
    let push2 = |out: &mut Vec<String>, flag: &str, path: &str| {
        out.push(flag.into());
        out.push(path.into());
        out.push(path.into());
    };
    // Writable roots (re-bound rw on top of the read-only root).
    for w in &policy.writable {
        push2(&mut out, "--bind", &w.to_string_lossy());
    }
    // Exec-surface files stay read-only even inside a writable parent.
    // `--ro-bind-try` tolerates a not-yet-created surface.
    for d in &policy.deny_write {
        push2(&mut out, "--ro-bind-try", &d.to_string_lossy());
    }
    // Credential denies: mask an existing directory with an empty tmpfs,
    // and shadow an existing file with /dev/null — both hide the content
    // (no read) and block writes. bwrap requires the mount destination to
    // exist, so a credential path that isn't present is skipped: the
    // sandboxed process can't create it anyway (its parent lives under the
    // read-only root), so there's nothing to deny.
    for d in &policy.deny_read_write {
        let s = d.to_string_lossy().into_owned();
        if d.is_dir() {
            out.push("--tmpfs".into());
            out.push(s);
        } else if d.exists() {
            out.push("--ro-bind".into());
            out.push("/dev/null".into());
            out.push(s);
        }
    }
    for flag in [
        "--unshare-user",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
    ] {
        out.push(flag.into());
    }
    if !policy.allow_network {
        out.push("--unshare-net".into());
    }
    out.push("--".into());
    out.push(program.to_owned());
    out.extend(args.iter().cloned());
    out
}

/// Whether the Linux sandbox engine is usable here: run a trivial sandboxed
/// `/bin/true` and require success. Not a `which` check — unprivileged user
/// namespaces are restricted on some distros (Ubuntu's AppArmor, hardened
/// kernels), so the mechanism can be installed yet unusable; this exercises
/// it. Cheap, quota-free; the discovery layer caches the result.
#[cfg(target_os = "linux")]
pub fn bwrap_available() -> bool {
    std::process::Command::new(bwrap_program())
        .args(["--ro-bind", "/", "/", "--unshare-user", "--", "/bin/true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub fn bwrap_available() -> bool {
    false
}

/// Central credential directories/files denied read+write to any sandboxed
/// lead. Tilde-relative; expanded against `$HOME` at build time. The lead's
/// *own* harness credentials are layered back in as readable by the caller.
pub const CREDENTIAL_DENY: &[&str] = &[
    "~/.ssh",
    "~/.aws",
    "~/.gnupg",
    "~/.config/gh",
    "~/.config/gcloud",
    "~/.azure",
    "~/.kube",
    "~/.netrc",
    "~/.npmrc",
    "~/.docker/config.json",
];

/// Basenames that must stay unwritable inside any granted harness state dir:
/// the execute-and-instruction surfaces whose contents run (or steer a
/// model) on the user's *next*, unsandboxed run. Joined under each writable
/// state dir as an explicit deny-write overlay.
pub const EXEC_SURFACE_NAMES: &[&str] = &[
    "hooks.json",
    "mcp.json",
    "mcp-config.json",
    "permissions-config.json",
    "settings.json",
    "config.json",
    "skills",
    "plugins",
    "extensions",
    "installed-plugins",
    "instructions",
    "agents",
    "bin",
];

/// Credential environment variables stripped from a sandboxed lead's child
/// process. Exact names plus prefixes (`AWS_`, …); a per-harness keep-list
/// (e.g. Copilot's `GH_TOKEN`) is subtracted by [`credential_env_removals`].
const CREDENTIAL_ENV_EXACT: &[&str] = &[
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "COPILOT_GITHUB_TOKEN",
    "SSH_AUTH_SOCK",
    "NPM_TOKEN",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
];
const CREDENTIAL_ENV_PREFIXES: &[&str] = &["AWS_", "GOOGLE_", "AZURE_"];

/// Expand a leading `~` against `$HOME`. Non-tilde paths pass through.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// The concrete credential env keys to remove from a sandboxed lead's child,
/// resolved against the current process env (prefix matches expand to actual
/// keys) minus the harness's keep-list.
pub fn credential_env_removals(keep: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy().into_owned();
        if keep.contains(&key.as_str()) {
            continue;
        }
        let hit = CREDENTIAL_ENV_EXACT.contains(&key.as_str())
            || CREDENTIAL_ENV_PREFIXES.iter().any(|p| key.starts_with(p));
        if hit {
            out.push(key);
        }
    }
    out
}

/// Inputs for assembling a lead's sandbox policy. Paths are tilde-relative
/// where noted; the builder expands, creates, and canonicalizes them.
pub struct LeadSpecInputs<'a> {
    pub engine: SandboxEngine,
    /// The project directory whose `.mix2/` the lead may write.
    pub cwd: &'a Path,
    /// The per-session runtime dir; a `lead-tmp` subdir under it becomes the
    /// only broadly-writable scratch (also the child's `TMPDIR`).
    pub runtime_dir: &'a Path,
    /// The leading harness's own state dirs (tilde-relative), granted write.
    pub state_dirs: &'a [&'a str],
    /// Other harnesses' credential files (tilde-relative), denied read+write
    /// so one harness can't exfiltrate another's token.
    pub other_credential_files: &'a [&'a str],
    /// Env vars this harness must keep despite the credential strip.
    pub env_keep: &'a [&'a str],
}

/// Assemble the sandbox spec for a lead invocation: writable roots (`.mix2`,
/// the lead-tmp scratch, the harness state dirs), an exec-surface deny-write
/// overlay inside those state dirs, credential deny-read, and the env strip.
/// The returned `lead_tmp` is the dir the caller should point `TMPDIR` at.
pub fn build_lead_spec(inputs: LeadSpecInputs<'_>) -> std::io::Result<(SandboxSpec, PathBuf)> {
    let mix2 = prepare_writable_root(&inputs.cwd.join(".mix2"), true)?;
    let lead_tmp = prepare_writable_root(&inputs.runtime_dir.join("lead-tmp"), true)?;

    let mut writable = vec![mix2, lead_tmp.clone()];
    let mut deny_write: Vec<PathBuf> = Vec::new();
    for dir in inputs.state_dirs {
        let expanded = expand_tilde(dir);
        // Skip a state dir we can't materialize as a real dir (e.g. a
        // symlink, or an un-creatable path) rather than failing the whole
        // lead — a missing optional cache dir shouldn't block coordination.
        let Ok(root) = prepare_writable_root(&expanded, true) else {
            continue;
        };
        // Overlay the exec-surface denies inside this granted state dir.
        for name in EXEC_SURFACE_NAMES {
            deny_write.push(root.join(name));
        }
        writable.push(root);
    }

    let mut deny_read_write: Vec<PathBuf> =
        CREDENTIAL_DENY.iter().map(|p| expand_tilde(p)).collect();
    for cred in inputs.other_credential_files {
        deny_read_write.push(expand_tilde(cred));
    }

    let spec = SandboxSpec {
        engine: inputs.engine,
        policy: SandboxPolicy {
            writable,
            deny_write,
            deny_read_write,
            allow_network: true,
        },
        env_remove: credential_env_removals(inputs.env_keep),
    };
    Ok((spec, lead_tmp))
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
        // Anchor each clause on a marker unique to it: the bare withdraw
        // line and the per-block param references (WRITE0 / DENYW0 /
        // DENYRW0 each appear in exactly one block). The clause headers
        // `(deny file-write*)` and `(deny file-write*\n` differ by a single
        // character, so keying on them is too fragile to prove ordering.
        let allow_default = profile.find("(allow default)").unwrap();
        let deny_all_writes = profile.find("(deny file-write*)\n").unwrap();
        let allow_writes = profile.find("WRITE0").unwrap();
        let deny_exec = profile.find("DENYW0").unwrap();
        let deny_creds = profile.find("DENYRW0").unwrap();
        let deny_appleevents = profile.find("(deny appleevent-send)").unwrap();
        // Last match wins, so the withdraw precedes the re-grant, and the
        // exec-surface and credential denies come after the write grant.
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
    fn bwrap_argv_binds_writable_and_masks_denies() {
        // Use real temp paths so the dir-vs-file dispatch (tmpfs vs
        // /dev/null) is deterministic.
        let dir = tempfile::tempdir().unwrap();
        let mix2 = dir.path().join(".mix2");
        std::fs::create_dir(&mix2).unwrap();
        let creddir = dir.path().join("creddir");
        std::fs::create_dir(&creddir).unwrap();
        let credfile = dir.path().join("token.json");
        std::fs::write(&credfile, "x").unwrap();
        let exec_surface = mix2.join("hooks.json");

        let missing = dir.path().join("does-not-exist");
        let policy = SandboxPolicy {
            writable: vec![mix2.clone()],
            deny_write: vec![exec_surface.clone()],
            deny_read_write: vec![creddir.clone(), credfile.clone(), missing.clone()],
            allow_network: true,
        };
        let (program, args) = wrap(
            SandboxEngine::Bwrap,
            &policy,
            "/usr/bin/opencode",
            &["run".to_owned()],
        );
        assert!(program.ends_with("bwrap"));
        let joined = args.join(" ");
        // Base: read-only root (incl. /tmp, so it is never a blanket
        // writable escape), no netns unshare (network open).
        assert!(joined.contains("--ro-bind / /"));
        assert!(!joined.contains("--tmpfs /tmp"));
        assert!(!joined.contains("--unshare-net"));
        assert!(joined.contains("--new-session"));
        // Writable re-bind, exec-surface ro-bind, dir mask via tmpfs, file
        // mask via /dev/null.
        let m = mix2.to_string_lossy();
        assert!(joined.contains(&format!("--bind {m} {m}")));
        let ex = exec_surface.to_string_lossy();
        assert!(joined.contains(&format!("--ro-bind-try {ex} {ex}")));
        let cd = creddir.to_string_lossy();
        assert!(joined.contains(&format!("--tmpfs {cd}")));
        let cf = credfile.to_string_lossy();
        assert!(joined.contains(&format!("--ro-bind /dev/null {cf}")));
        // A non-existent deny path is skipped — bwrap can't mount onto a
        // missing destination, and the sandboxed process can't create it
        // under the read-only root anyway.
        assert!(!joined.contains(&missing.to_string_lossy().into_owned()));
        // The real command follows the `--` separator verbatim.
        let sep = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[sep + 1], "/usr/bin/opencode");
        assert_eq!(args[sep + 2], "run");
    }

    #[test]
    fn bwrap_unshares_net_only_when_network_withdrawn() {
        let mut p = SandboxPolicy::with_writable(vec![]);
        assert!(!wrap(SandboxEngine::Bwrap, &p, "/bin/true", &[])
            .1
            .contains(&"--unshare-net".to_owned()));
        p.allow_network = false;
        assert!(wrap(SandboxEngine::Bwrap, &p, "/bin/true", &[])
            .1
            .contains(&"--unshare-net".to_owned()));
    }

    /// Behavioral denial matrix for Linux, mirroring the macOS one. Runs real
    /// `bwrap`; skipped where user namespaces are unavailable (containers,
    /// hardened kernels). This is the load-bearing proof on Linux — the
    /// golden tests only prove the argv shape.
    #[cfg(target_os = "linux")]
    #[test]
    fn bwrap_behavioral_denial_matrix() {
        use std::process::Stdio;
        if !bwrap_available() {
            eprintln!("skipping: bwrap/userns unavailable on this host");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().canonicalize().unwrap();
        let mix2 = prepare_writable_root(&proj.join(".mix2"), true).unwrap();
        // Include credential denies: an existing dir + file, and a
        // deliberately non-existent path — the latter must be skipped, or
        // bwrap fails to mount and the whole lead won't start.
        let creddir = proj.join("creds");
        std::fs::create_dir(&creddir).unwrap();
        std::fs::write(creddir.join("token"), "secret").unwrap();
        let policy = SandboxPolicy {
            writable: vec![mix2.clone()],
            deny_write: vec![],
            deny_read_write: vec![creddir.clone(), proj.join("nonexistent-cred")],
            allow_network: true,
        };
        let run = |script: String| -> bool {
            let (program, args) = wrap(
                SandboxEngine::Bwrap,
                &policy,
                "/bin/sh",
                &["-c".to_owned(), script],
            );
            std::process::Command::new(program)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        // The lead starts despite the non-existent deny path, and the
        // in-scope write is allowed.
        assert!(
            run(format!("echo ok > {}/in.txt", mix2.display())),
            "write inside .mix2 must be allowed (and a missing deny path must not break startup)"
        );
        let escape = proj.join("escape.txt");
        assert!(
            !run(format!("echo bad > {}", escape.display())),
            "write outside .mix2 must be denied"
        );
        // The masked credential is unreadable.
        assert!(
            !run(format!("cat {}/token", creddir.display())),
            "reading the masked credential dir must be denied"
        );
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

    #[test]
    fn build_lead_spec_assembles_writable_denies_and_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("proj");
        std::fs::create_dir(&cwd).unwrap();
        let runtime = dir.path().join("rt");
        std::fs::create_dir(&runtime).unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let state_str = state.to_string_lossy().into_owned();

        let (spec, lead_tmp) = build_lead_spec(LeadSpecInputs {
            engine: SandboxEngine::Seatbelt,
            cwd: &cwd,
            runtime_dir: &runtime,
            state_dirs: &[state_str.as_str()],
            other_credential_files: &["~/.local/share/opencode/auth.json"],
            env_keep: &[],
        })
        .unwrap();

        // Writable: .mix2, the lead-tmp scratch, and the state dir.
        let writable: Vec<_> = spec.policy.writable.iter().collect();
        assert!(writable.iter().any(|p| p.ends_with(".mix2")));
        assert!(writable.iter().any(|p| p.ends_with("lead-tmp")));
        assert!(writable
            .iter()
            .any(|p| p.canonicalize().ok() == state.canonicalize().ok()));
        assert!(lead_tmp.ends_with("lead-tmp"));

        // Exec-surface deny overlay lives inside the granted state dir.
        assert!(spec
            .policy
            .deny_write
            .iter()
            .any(|p| p.ends_with("hooks.json")));

        // Credential denies: central set plus the other harness's auth file.
        assert!(spec
            .policy
            .deny_read_write
            .iter()
            .any(|p| p.ends_with(".ssh")));
        assert!(spec
            .policy
            .deny_read_write
            .iter()
            .any(|p| p.ends_with("opencode/auth.json")));
    }

    #[cfg(unix)]
    #[test]
    fn build_lead_spec_fails_when_mix2_is_a_symlink() {
        // A symlinked `.mix2` can't be safely granted (it would redirect
        // writes to the target), so assembly fails — the runtime then fails
        // the turn rather than running a sandbox-only lead unconfined.
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("proj");
        std::fs::create_dir(&cwd).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, cwd.join(".mix2")).unwrap();
        let runtime = dir.path().join("rt");
        std::fs::create_dir(&runtime).unwrap();

        let result = build_lead_spec(LeadSpecInputs {
            engine: SandboxEngine::Seatbelt,
            cwd: &cwd,
            runtime_dir: &runtime,
            state_dirs: &[],
            other_credential_files: &[],
            env_keep: &[],
        });
        assert!(result.is_err(), "a symlinked .mix2 must fail assembly");
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
