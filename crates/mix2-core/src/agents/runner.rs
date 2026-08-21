//! The shared invocation runner: one [`Agent`] implementation driven by a
//! [`Descriptor`]. All process handling — spawn, stream decode, cancellation
//! (tree kill), stderr tails, failure shaping — lives here exactly once;
//! per-harness modules contribute only pure builders, probes, and decoders.

use super::agent::{Agent, AgentRequest, AgentResult, AgentSession, AgentVersion, AuthState};
use super::descriptor::{AuthProbe, Descriptor};
use super::{AgentEvent, HarnessKind};
use crate::process::child::{ChildProcess, SpawnOptions};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// A harness adapter: a descriptor plus the resolved command to invoke.
pub struct HarnessAgent {
    descriptor: &'static Descriptor,
    pub command: String,
}

impl HarnessAgent {
    pub fn new(descriptor: &'static Descriptor, command: impl Into<String>) -> Self {
        Self {
            descriptor,
            command: command.into(),
        }
    }

    async fn run(
        &self,
        request: AgentRequest,
        resume: Option<&str>,
        events: Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<AgentResult> {
        let label = self.descriptor.label;
        let base_args = (self.descriptor.build_args)(&request, resume);
        let mut env = request.env.clone();
        if let Some(prepend) = &request.path_prepend {
            let path = std::env::var("PATH").unwrap_or_default();
            env.insert("PATH".into(), format!("{}:{}", prepend.display(), path));
        }

        // Prompt delivery is descriptor-declared: positional-argument CLIs
        // (cursor) already carry it in `args`; the rest read stdin. Compute
        // it against the real command before any sandbox wrapping.
        let stdin = if self.descriptor.prompt_in_args {
            None
        } else {
            Some(request.prompt.as_str())
        };

        // Wrap in the OS sandbox when the request carries one (a lead whose
        // harness can't scope its own writes). The engine execs the real
        // command in place, so stdin/stdout and the process-group kill are
        // unaffected; the argv is otherwise byte-identical to the native
        // run, which is what makes an unsandboxed run a true rollback.
        let sandboxed = request.sandbox.is_some();
        let (program, args) = match &request.sandbox {
            Some(spec) => {
                crate::sandbox::wrap(spec.engine, &spec.policy, &self.command, &base_args)
            }
            None => (self.command.clone(), base_args),
        };
        let env_remove: &[String] = match &request.sandbox {
            Some(spec) => &spec.env_remove,
            None => &[],
        };

        let mut child = ChildProcess::spawn(SpawnOptions {
            program: &program,
            args: &args,
            cwd: &request.cwd,
            env: &env,
            env_remove,
            stdin,
        })?;

        let _ = events.send(AgentEvent::Started).await;

        let mut lines = child.stdout_lines()?;
        let stderr = child.stderr_tail()?;
        let mut decoder = (self.descriptor.new_decoder)();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    child.kill_tree().await;
                    bail!("cancelled");
                }
                line = lines.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            for ev in decoder.parse_line(&line) {
                                let _ = events.send(ev).await;
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!("{label} stdout read error: {e}");
                            break;
                        }
                    }
                }
            }
        }

        let status = child.wait().await?;
        let stderr_tail = stderr.await.unwrap_or_default();
        let outcome = decoder.finish();

        if let Some(err) = outcome.error {
            let _ = events
                .send(AgentEvent::Failed {
                    message: err.clone(),
                })
                .await;
            bail!("{label} failed: {err}");
        }
        if !status.success() && outcome.final_text.is_none() {
            let msg = friendly_failure(label, &status, &stderr_tail, sandboxed);
            let _ = events
                .send(AgentEvent::Failed {
                    message: msg.clone(),
                })
                .await;
            bail!("{msg}");
        }

        let _ = events.send(AgentEvent::Completed).await;
        Ok(AgentResult {
            text: outcome.final_text.unwrap_or(outcome.fallback_text),
            session_id: outcome.session_id,
        })
    }
}

pub(crate) fn friendly_failure(
    provider: &str,
    status: &std::process::ExitStatus,
    stderr_tail: &str,
    sandboxed: bool,
) -> String {
    let tail = stderr_tail.trim();
    // A sandboxed lead whose harness has started self-sandboxing collides
    // with the outer engine: macOS reports `sandbox_apply: Operation not
    // permitted` and exit 71. Attribute that to the sandbox so it never
    // reads as a harness bug, and so the canary is obvious in logs.
    if sandboxed
        && (tail.contains("sandbox_apply: Operation not permitted") || status.code() == Some(71))
    {
        return format!(
            "sandbox: {provider} could not start under the mix2 sandbox — it appears to apply \
             its own sandbox, which cannot nest. Run it as the teammate, or turn the mix2 \
             sandbox off for this harness. ({status})"
        );
    }
    // The engine itself refusing (bad profile, missing binary, user
    // namespaces denied) prints its own diagnostic; surface it as a sandbox
    // failure, not a harness one. Covers both engines' error prefixes.
    if sandboxed
        && (tail.contains("sandbox-exec:")
            || tail.contains("bwrap:")
            || tail.contains("setting up uid map")
            || tail.contains("user namespace"))
    {
        return format!("sandbox: engine error starting {provider}: {tail} ({status})");
    }
    // Otherwise the harness ran under the sandbox and failed on its own
    // terms — its message stands, no misleading sandbox prefix.
    if tail.is_empty() {
        format!("{provider} exited with {status}")
    } else {
        format!("{provider} exited with {status}: {tail}")
    }
}

#[async_trait]
impl Agent for HarnessAgent {
    fn harness(&self) -> HarnessKind {
        self.descriptor.harness
    }

    async fn version(&self) -> Result<AgentVersion> {
        let out = tokio::process::Command::new(&self.command)
            .args(self.descriptor.version_args)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .with_context(|| format!("`{}` not found or not executable", self.command))?;
        if !out.status.success() {
            bail!(
                "`{} {}` failed",
                self.command,
                self.descriptor.version_args.join(" ")
            );
        }
        let raw = String::from_utf8_lossy(&out.stdout);
        Ok(AgentVersion {
            raw: (self.descriptor.parse_version)(&raw),
        })
    }

    fn known_models(&self) -> Vec<String> {
        self.descriptor
            .known_models
            .iter()
            .map(|m| (*m).to_owned())
            .collect()
    }

    async fn models(&self) -> Vec<String> {
        let Some(args) = self.descriptor.models_args else {
            return self.known_models();
        };
        // Live, quota-free enumeration, bounded so a harness exposing an
        // enormous catalog can't flood the picker. Any failure degrades to
        // the curated fallback — never to unavailability.
        const MAX_MODELS: usize = 50;
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            tokio::process::Command::new(&self.command)
                .args(args)
                .stdin(std::process::Stdio::null())
                .output(),
        )
        .await;
        let Ok(Ok(out)) = out else {
            return self.known_models();
        };
        if !out.status.success() {
            return self.known_models();
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let models: Vec<String> = stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.contains(' '))
            .take(MAX_MODELS)
            .map(str::to_owned)
            .collect();
        if models.is_empty() {
            self.known_models()
        } else {
            models
        }
    }

    async fn auth_status(&self) -> AuthState {
        let args = match self.descriptor.auth_probe {
            AuthProbe::JsonLoggedIn { args }
            | AuthProbe::ExitStatus { args }
            | AuthProbe::CredentialInventory { args } => args,
            // No probe exists — never burn quota with trial prompts; surface
            // run-time auth failures cleanly instead.
            AuthProbe::None => return AuthState::Unsupported,
        };
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::process::Command::new(&self.command)
                .args(args)
                .stdin(std::process::Stdio::null())
                .output(),
        )
        .await;
        let Ok(Ok(out)) = out else {
            return AuthState::ProbeFailed;
        };
        match self.descriptor.auth_probe {
            AuthProbe::None => unreachable!("handled above"),
            AuthProbe::ExitStatus { .. } => {
                if out.status.success() {
                    AuthState::Authenticated
                } else {
                    AuthState::Unauthenticated
                }
            }
            AuthProbe::CredentialInventory { .. } => {
                // An inventory proves credentials exist, not that they are
                // live — Configured, never Authenticated. A failing
                // inventory proves nothing either way.
                if out.status.success() {
                    AuthState::Configured
                } else {
                    AuthState::ProbeFailed
                }
            }
            AuthProbe::JsonLoggedIn { .. } => {
                // Shell hooks may prepend banner lines; scan from the first
                // brace before parsing.
                let stdout = String::from_utf8_lossy(&out.stdout);
                let json_start = match stdout.find('{') {
                    Some(i) => i,
                    None => return AuthState::ProbeFailed,
                };
                match serde_json::from_str::<Value>(&stdout[json_start..]) {
                    Ok(v) => match v.get("loggedIn").and_then(Value::as_bool) {
                        Some(true) => AuthState::Authenticated,
                        Some(false) => AuthState::Unauthenticated,
                        None => AuthState::ProbeFailed,
                    },
                    Err(_) => AuthState::ProbeFailed,
                }
            }
        }
    }

    async fn start(
        &self,
        request: AgentRequest,
        events: Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<AgentResult> {
        self.run(request, None, events, cancel).await
    }

    async fn resume(
        &self,
        session: &AgentSession,
        request: AgentRequest,
        events: Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<AgentResult> {
        self.run(request, Some(&session.id), events, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::descriptor::{
        AuthProbe, Capabilities, CapabilityLevel, DecodeOutcome, Decoder,
    };
    use crate::agents::AgentRole;
    use std::process::ExitStatus;

    // Synthesizes an ExitStatus carrying a specific code. Unix-only: it
    // relies on `ExitStatusExt::from_raw`, and there is no portable way to
    // construct an arbitrary exit code. mix2-core is Unix-only anyway
    // (`mix2-consult` uses `std::os::unix::net`), so the failure-attribution
    // tests that need a specific code are `#[cfg(unix)]`.
    #[cfg(unix)]
    fn status_from_code(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }

    #[cfg(unix)]
    #[test]
    fn friendly_failure_attributes_nested_sandbox_to_the_engine() {
        // Exit 71 (or the macOS signature) under a sandbox is the harness
        // self-sandboxing and colliding with the outer engine.
        let msg = friendly_failure("cursor", &status_from_code(71), "", true);
        assert!(msg.starts_with("sandbox:"), "{msg}");
        assert!(msg.contains("cannot nest"), "{msg}");
    }

    #[cfg(unix)]
    #[test]
    fn friendly_failure_attributes_bwrap_engine_errors() {
        // A bwrap startup error (e.g. userns denied) is a sandbox failure,
        // not a harness one.
        let msg = friendly_failure(
            "opencode",
            &status_from_code(1),
            "bwrap: setting up uid map: Permission denied",
            true,
        );
        assert!(msg.starts_with("sandbox:"), "{msg}");
    }

    #[cfg(unix)]
    #[test]
    fn friendly_failure_does_not_blame_the_sandbox_for_a_harness_error() {
        // A harness that ran fine under the sandbox and failed on its own
        // terms keeps its own message — no misleading sandbox prefix.
        let msg = friendly_failure("cursor", &status_from_code(1), "model refused", true);
        assert!(!msg.starts_with("sandbox:"), "{msg}");
        assert!(msg.contains("model refused"), "{msg}");
    }

    #[cfg(unix)]
    #[test]
    fn friendly_failure_unsandboxed_is_unchanged() {
        let msg = friendly_failure("codex", &status_from_code(2), "boom", false);
        assert_eq!(
            msg,
            format!("codex exited with {}: boom", status_from_code(2))
        );
    }

    // A minimal harness whose "prompt" is a shell script: build_args runs it
    // via `/bin/sh -c`, and the decoder is a no-op so success/failure is
    // driven purely by the child's exit status. Used to prove the runner
    // actually spawns the harness under the sandbox engine.
    struct NoopDecoder;
    impl Decoder for NoopDecoder {
        fn parse_line(&mut self, _line: &str) -> Vec<AgentEvent> {
            Vec::new()
        }
        fn finish(&mut self) -> DecodeOutcome {
            DecodeOutcome::default()
        }
    }

    fn script_args(request: &AgentRequest, _resume: Option<&str>) -> Vec<String> {
        vec!["-c".to_owned(), request.prompt.clone()]
    }

    static SCRIPT_DESCRIPTOR: Descriptor = Descriptor {
        harness: HarnessKind::Cursor,
        label: "script",
        default_command: "/bin/sh",
        aliases: &[],
        command_env_override: "MIX2_SCRIPT_CMD_UNUSED",
        install_hint: "",
        login_hint: "",
        selection_note: None,
        prompt_in_args: true,
        capabilities: Capabilities {
            teammate_read_only: CapabilityLevel::Enforced,
            lead_permission_scoping: CapabilityLevel::Unsupported,
            instruction_injection: CapabilityLevel::Enforced,
        },
        sandboxable_lead: true,
        state_dirs: &[],
        credential_files: &[],
        env_keep_sandboxed: &[],
        known_models: &[],
        models_args: None,
        version_args: &["--version"],
        parse_version: |_| "0".to_owned(),
        auth_probe: AuthProbe::None,
        build_args: script_args,
        new_decoder: || Box::new(NoopDecoder),
    };

    async fn run_script(
        script: String,
        sandbox: Option<crate::sandbox::SandboxSpec>,
    ) -> Result<()> {
        let agent = HarnessAgent::new(&SCRIPT_DESCRIPTOR, "/bin/sh");
        let request = AgentRequest {
            prompt: script,
            cwd: std::env::temp_dir(),
            role: AgentRole::Lead,
            turn_id: uuid::Uuid::nil(),
            model: None,
            instructions: String::new(),
            env: std::collections::HashMap::new(),
            path_prepend: None,
            runtime_dir: None,
            sandbox,
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        agent
            .run(request, None, tx, CancellationToken::new())
            .await
            .map(|_| ())
    }

    /// The end-to-end proof for this PR: a lead command spawned through the
    /// runner with a sandbox spec is actually confined — a write outside the
    /// granted root is denied by the kernel, and the same script with no
    /// sandbox succeeds. This is what ties build_args + wrap + spawn
    /// together; the pure tests cover each piece in isolation. Engine-
    /// agnostic: runs under whichever engine the host provides (Seatbelt on
    /// macOS, bubblewrap on Linux), skipping where none is available.
    #[cfg(unix)]
    #[tokio::test]
    async fn sandboxed_lead_write_outside_scope_is_denied_by_the_runner() {
        use crate::sandbox::{prepare_writable_root, SandboxPolicy, SandboxSpec};
        let Some(engine) = SandboxSpec::detect_engine() else {
            eprintln!("skipping: no sandbox engine available on this host");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().canonicalize().unwrap();
        let mix2 = prepare_writable_root(&proj.join(".mix2"), true).unwrap();
        let spec = SandboxSpec {
            engine,
            policy: SandboxPolicy::with_writable(vec![mix2.clone()]),
            env_remove: Vec::new(),
        };

        // Allowed: write inside the granted root, under the sandbox.
        run_script(
            format!("echo ok > {}/in.txt", mix2.display()),
            Some(spec.clone()),
        )
        .await
        .expect("write inside .mix2 should succeed under the sandbox");

        // Denied: write outside the granted root fails the invocation, and
        // the escape file is never created.
        let escape = proj.join("escape.txt");
        let denied = run_script(format!("echo bad > {}", escape.display()), Some(spec)).await;
        assert!(denied.is_err(), "write outside .mix2 must be denied");
        assert!(!escape.exists(), "the out-of-scope file must not exist");

        // Control: with no sandbox the same out-of-scope write succeeds,
        // proving the denial came from the sandbox, not the script.
        let escape2 = proj.join("escape2.txt");
        run_script(format!("echo ok > {}", escape2.display()), None)
            .await
            .expect("unsandboxed write should succeed");
        assert!(escape2.exists());
    }
}
