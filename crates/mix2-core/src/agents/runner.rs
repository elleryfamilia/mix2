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
        let args = (self.descriptor.build_args)(&request, resume);
        let mut env = request.env.clone();
        if let Some(prepend) = &request.path_prepend {
            let path = std::env::var("PATH").unwrap_or_default();
            env.insert("PATH".into(), format!("{}:{}", prepend.display(), path));
        }

        // Prompt delivery is descriptor-declared: positional-argument CLIs
        // (cursor) already carry it in `args`; the rest read stdin.
        let stdin = if self.descriptor.prompt_in_args {
            None
        } else {
            Some(request.prompt.as_str())
        };
        let mut child = ChildProcess::spawn(SpawnOptions {
            program: &self.command,
            args: &args,
            cwd: &request.cwd,
            env: &env,
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
            let msg = friendly_failure(label, &status, &stderr_tail);
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
) -> String {
    let tail = stderr_tail.trim();
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

    async fn auth_status(&self) -> AuthState {
        let args = match self.descriptor.auth_probe {
            AuthProbe::JsonLoggedIn { args } | AuthProbe::ExitStatus { args } => args,
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
