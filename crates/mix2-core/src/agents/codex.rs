use super::agent::{Agent, AgentRequest, AgentResult, AgentSession, AgentVersion, AuthStatus};
use super::claude::friendly_failure;
use super::{AgentEvent, AgentKind, AgentRole};
use crate::process::child::{ChildProcess, SpawnOptions};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// Adapter for the OpenAI Codex CLI.
///
/// Invocation shape (verified against codex-cli 0.146.x):
///   codex exec --json --skip-git-repo-check [-c developer_instructions=...] -
///   codex exec resume <thread-id> --json --skip-git-repo-check ... -
/// with the prompt on stdin and the working directory set on the child
/// process. `-c developer_instructions=...` layers mix2's role instructions
/// on top of Codex's built-in agent instructions per run, without touching
/// the user's `~/.codex/config.toml`.
///
/// Sandbox note: `codex exec` defaults to a read-only sandbox that blocks
/// both Unix-socket connects and all file writes (verified empirically), so
/// a Codex *lead* could never reach the consult helper. Leads therefore run
/// with `sandbox_mode="workspace-write"` — Codex's standard interactive
/// sandbox level (workspace-writable, still no network) — plus the mix2
/// runtime dir added to writable roots for consult file IPC. Teammate
/// consultations keep the user's default sandbox untouched.
pub struct CodexAgent {
    pub command: String,
}

impl CodexAgent {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }

    fn build_args(&self, request: &AgentRequest, resume: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = vec!["exec".into()];
        if let Some(id) = resume {
            args.push("resume".into());
            args.push(id.into());
        }
        args.push("--json".into());
        args.push("--skip-git-repo-check".into());
        if let Some(model) = &request.model {
            args.push("-c".into());
            args.push(format!("model={}", toml_string(model)));
        }
        if !request.instructions.is_empty() {
            args.push("-c".into());
            args.push(format!(
                "developer_instructions={}",
                toml_string(&request.instructions)
            ));
        }
        if request.role == AgentRole::Lead {
            args.push("-c".into());
            args.push("sandbox_mode=\"workspace-write\"".into());
            if let Some(rt) = &request.runtime_dir {
                args.push("-c".into());
                args.push(format!(
                    "sandbox_workspace_write.writable_roots=[{}]",
                    toml_string(&rt.display().to_string())
                ));
            }
        }
        args.push("-".into());
        args
    }

    async fn run(
        &self,
        request: AgentRequest,
        resume: Option<&str>,
        events: Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<AgentResult> {
        let args = self.build_args(&request, resume);
        let mut env = request.env.clone();
        if let Some(prepend) = &request.path_prepend {
            let path = std::env::var("PATH").unwrap_or_default();
            env.insert("PATH".into(), format!("{}:{}", prepend.display(), path));
        }

        let mut child = ChildProcess::spawn(SpawnOptions {
            program: &self.command,
            args: &args,
            cwd: &request.cwd,
            env: &env,
            stdin: Some(&request.prompt),
        })?;

        let _ = events
            .send(AgentEvent::Started { agent: self.kind() })
            .await;

        let mut lines = child.stdout_lines()?;
        let stderr = child.stderr_tail()?;
        let mut parser = CodexStreamParser::default();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    child.kill_tree().await;
                    bail!("cancelled");
                }
                line = lines.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            for ev in parser.parse_line(&line) {
                                let _ = events.send(ev).await;
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!("codex stdout read error: {e}");
                            break;
                        }
                    }
                }
            }
        }

        let status = child.wait().await?;
        let stderr_tail = stderr.await.unwrap_or_default();

        if let Some(err) = parser.error.take() {
            let _ = events
                .send(AgentEvent::Failed {
                    agent: self.kind(),
                    message: err.clone(),
                })
                .await;
            bail!("codex failed: {err}");
        }
        if !status.success() && parser.final_text.is_none() {
            let msg = friendly_failure("codex", &status, &stderr_tail);
            let _ = events
                .send(AgentEvent::Failed {
                    agent: self.kind(),
                    message: msg.clone(),
                })
                .await;
            bail!("{msg}");
        }

        let _ = events
            .send(AgentEvent::Completed { agent: self.kind() })
            .await;
        Ok(AgentResult {
            text: parser.final_text.take().unwrap_or_default(),
            session_id: parser.thread_id.clone(),
        })
    }
}

/// Encode a string as a TOML basic string (codex `-c` values are parsed as
/// TOML, and instructions contain newlines and quotes).
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[async_trait]
impl Agent for CodexAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    async fn version(&self) -> Result<AgentVersion> {
        let out = tokio::process::Command::new(&self.command)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .with_context(|| format!("`{}` not found or not executable", self.command))?;
        if !out.status.success() {
            bail!("`{} --version` failed", self.command);
        }
        let raw = String::from_utf8_lossy(&out.stdout);
        let line = raw
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("unknown");
        Ok(AgentVersion {
            raw: line.to_owned(),
        })
    }

    async fn auth_status(&self) -> AuthStatus {
        // `codex login status` exits 0 when signed in, non-zero otherwise.
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::process::Command::new(&self.command)
                .args(["login", "status"])
                .stdin(std::process::Stdio::null())
                .output(),
        )
        .await;
        match out {
            Ok(Ok(out)) if out.status.success() => AuthStatus::Authenticated,
            Ok(Ok(_)) => AuthStatus::Unauthenticated,
            _ => AuthStatus::Unknown,
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

/// Tolerant parser for `codex exec --json` JSONL events.
/// Unknown item and event types must never panic the application.
#[derive(Default)]
pub struct CodexStreamParser {
    pub thread_id: Option<String>,
    pub final_text: Option<String>,
    pub error: Option<String>,
    /// Bytes of text already emitted per agent_message item id. Codex sends
    /// cumulative snapshots; tracking only the emitted length keeps delta
    /// extraction O(delta) instead of O(total²) over a long answer.
    message_progress: HashMap<String, usize>,
    running_commands: HashMap<String, String>,
}

impl CodexStreamParser {
    pub fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let agent = AgentKind::Codex;
        let line = line.trim();
        if line.is_empty() {
            return vec![];
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                return vec![AgentEvent::ParserWarning {
                    agent,
                    message: format!("unparseable line ({} bytes)", line.len()),
                }]
            }
        };

        let mut out = Vec::new();
        match value.get("type").and_then(Value::as_str) {
            Some("thread.started") => {
                if let Some(id) = value.get("thread_id").and_then(Value::as_str) {
                    self.thread_id = Some(id.to_owned());
                    out.push(AgentEvent::SessionStarted {
                        agent,
                        session_id: id.to_owned(),
                    });
                }
            }
            Some("turn.started") => {}
            Some("turn.completed") => {
                if let Some(usage) = value.get("usage") {
                    out.push(AgentEvent::Usage {
                        agent,
                        input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
                        output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
                    });
                }
            }
            Some("turn.failed") => {
                let msg = value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("codex turn failed")
                    .to_owned();
                self.error = Some(msg);
            }
            Some("error") => {
                let msg = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("codex reported an error")
                    .to_owned();
                self.error = Some(msg);
            }
            Some(t @ ("item.started" | "item.updated" | "item.completed")) => {
                let item = value.get("item").cloned().unwrap_or(Value::Null);
                let item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                match item.get("type").and_then(Value::as_str) {
                    Some("agent_message") => {
                        let text = item.get("text").and_then(Value::as_str).unwrap_or("");
                        let seen = self.message_progress.entry(item_id).or_default();
                        // Snapshots are cumulative; emit only the new suffix.
                        // A shrinking or rewritten snapshot (never observed,
                        // but tolerated) resets without emitting garbage.
                        match text.get(*seen..) {
                            Some(delta) if !delta.is_empty() => {
                                out.push(AgentEvent::TextDelta {
                                    agent,
                                    text: delta.to_owned(),
                                });
                            }
                            _ => {}
                        }
                        *seen = text.len();
                        if t == "item.completed" {
                            self.final_text = Some(text.to_owned());
                            out.push(AgentEvent::Message {
                                agent,
                                text: text.to_owned(),
                            });
                        }
                    }
                    Some("command_execution") => {
                        let command = item
                            .get("command")
                            .and_then(Value::as_str)
                            .unwrap_or("shell")
                            .to_owned();
                        if t == "item.completed" {
                            self.running_commands.remove(&item_id);
                            out.push(AgentEvent::ToolFinished {
                                agent,
                                name: "shell".into(),
                            });
                        } else if let std::collections::hash_map::Entry::Vacant(entry) =
                            self.running_commands.entry(item_id)
                        {
                            entry.insert(command.clone());
                            out.push(AgentEvent::ToolStarted {
                                agent,
                                name: "shell".into(),
                                detail: Some(short_detail(&command)),
                            });
                        }
                    }
                    Some("file_change") => {
                        if t == "item.completed" {
                            out.push(AgentEvent::ToolFinished {
                                agent,
                                name: "edit".into(),
                            });
                        } else {
                            let detail = item
                                .get("changes")
                                .and_then(Value::as_array)
                                .map(|c| format!("{} file(s)", c.len()));
                            out.push(AgentEvent::ToolStarted {
                                agent,
                                name: "edit".into(),
                                detail,
                            });
                        }
                    }
                    Some("web_search") => {
                        if t == "item.completed" {
                            out.push(AgentEvent::ToolFinished {
                                agent,
                                name: "search".into(),
                            });
                        } else {
                            out.push(AgentEvent::ToolStarted {
                                agent,
                                name: "search".into(),
                                detail: item
                                    .get("query")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            });
                        }
                    }
                    // `reasoning` items are intentionally ignored: hidden
                    // reasoning is never exposed to the UI.
                    Some("reasoning") => {}
                    _ => {}
                }
            }
            Some(_) | None => {}
        }
        out
    }
}

fn short_detail(command: &str) -> String {
    let flat = command.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 80;
    if flat.chars().count() > MAX {
        let truncated: String = flat.chars().take(MAX - 1).collect();
        format!("{truncated}…")
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_thread_id() {
        let mut p = CodexStreamParser::default();
        let ev = p.parse_line(r#"{"type":"thread.started","thread_id":"01a00224-aaaa"}"#);
        assert_eq!(
            ev,
            vec![AgentEvent::SessionStarted {
                agent: AgentKind::Codex,
                session_id: "01a00224-aaaa".into()
            }]
        );
    }

    #[test]
    fn parses_final_message() {
        let mut p = CodexStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"OK"}}"#,
        );
        assert!(ev.contains(&AgentEvent::Message {
            agent: AgentKind::Codex,
            text: "OK".into()
        }));
        assert_eq!(p.final_text.as_deref(), Some("OK"));
    }

    #[test]
    fn incremental_agent_message_produces_deltas() {
        let mut p = CodexStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"item.updated","item":{"id":"m1","type":"agent_message","text":"Hel"}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::TextDelta {
                agent: AgentKind::Codex,
                text: "Hel".into()
            }]
        );
        let ev = p.parse_line(
            r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"Hello"}}"#,
        );
        assert_eq!(ev.len(), 2); // delta "lo" + full message
        assert_eq!(p.final_text.as_deref(), Some("Hello"));
    }

    #[test]
    fn parses_command_execution_lifecycle() {
        let mut p = CodexStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"item.started","item":{"id":"c1","type":"command_execution","command":"cargo test"}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::ToolStarted {
                agent: AgentKind::Codex,
                name: "shell".into(),
                detail: Some("cargo test".into())
            }]
        );
        let ev = p.parse_line(
            r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"cargo test","exit_code":0}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::ToolFinished {
                agent: AgentKind::Codex,
                name: "shell".into()
            }]
        );
    }

    #[test]
    fn reasoning_items_are_never_surfaced() {
        let mut p = CodexStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"item.completed","item":{"id":"r1","type":"reasoning","text":"hidden"}}"#,
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn unknown_item_and_event_types_tolerated() {
        let mut p = CodexStreamParser::default();
        assert!(p
            .parse_line(r#"{"type":"item.completed","item":{"id":"x","type":"hologram_call"}}"#)
            .is_empty());
        assert!(p
            .parse_line(r#"{"type":"future.event","data":[1,2]}"#)
            .is_empty());
    }

    #[test]
    fn malformed_line_yields_warning() {
        let mut p = CodexStreamParser::default();
        let ev = p.parse_line("garbage{{{");
        assert!(matches!(ev.as_slice(), [AgentEvent::ParserWarning { .. }]));
    }

    #[test]
    fn turn_failed_sets_error() {
        let mut p = CodexStreamParser::default();
        p.parse_line(r#"{"type":"turn.failed","error":{"message":"rate limited"}}"#);
        assert_eq!(p.error.as_deref(), Some("rate limited"));
    }

    #[test]
    fn toml_string_escapes() {
        assert_eq!(toml_string("a\"b\nc"), "\"a\\\"b\\nc\"");
    }
}
