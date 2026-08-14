use super::agent::{Agent, AgentRequest, AgentResult, AgentSession, AgentVersion};
use super::{AgentEvent, AgentKind, AgentRole};
use crate::process::child::{ChildProcess, SpawnOptions};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// Adapter for the Claude Code CLI.
///
/// Invocation shape (verified against claude 2.1.x):
///   claude -p --output-format stream-json --verbose --include-partial-messages \
///          --append-system-prompt <role instructions> [--resume <session-id>]
/// with the prompt on stdin. `--append-system-prompt` layers Cladex's role
/// instructions on top of Claude Code's own system prompt instead of
/// replacing it, and `--allowedTools Bash(cladex-consult:*)` permits exactly
/// the consult helper without widening anything else.
pub struct ClaudeAgent {
    pub command: String,
}

impl ClaudeAgent {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }

    fn build_args(&self, request: &AgentRequest, resume: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--include-partial-messages".into(),
            "--append-system-prompt".into(),
            request.instructions.clone(),
        ];
        if request.role == AgentRole::Lead {
            // The one targeted permission Cladex needs: the lead must be able
            // to run the consult helper. Everything else follows the user's
            // own Claude Code permission configuration.
            args.push("--allowedTools".into());
            args.push("Bash(cladex-consult:*)".into());
        }
        if let Some(id) = resume {
            args.push("--resume".into());
            args.push(id.into());
        }
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
        let mut parser = ClaudeStreamParser::default();

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
                            tracing::warn!("claude stdout read error: {e}");
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
            bail!("claude failed: {err}");
        }
        if !status.success() && parser.final_text.is_none() {
            let msg = friendly_failure("claude", &status, &stderr_tail);
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
            text: parser.take_final_text(),
            session_id: parser.session_id.clone(),
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
impl Agent for ClaudeAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    async fn version(&self) -> Result<AgentVersion> {
        let out = tokio::process::Command::new(&self.command)
            .arg("--version")
            .output()
            .await
            .with_context(|| format!("`{}` not found or not executable", self.command))?;
        if !out.status.success() {
            bail!("`{} --version` failed", self.command);
        }
        let raw = String::from_utf8_lossy(&out.stdout);
        // Loadout-style shell hooks can prepend banner lines; take the line
        // that actually looks like a version.
        let line = raw
            .lines()
            .map(str::trim)
            .find(|l| l.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .or_else(|| raw.lines().map(str::trim).find(|l| !l.is_empty()))
            .unwrap_or("unknown");
        Ok(AgentVersion {
            raw: line.to_owned(),
        })
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

/// Tolerant parser for `claude --output-format stream-json` lines.
/// Unknown event types are ignored (with a parser warning in debug logs);
/// they must never crash Cladex. Thinking deltas are consumed for state but
/// never surfaced as text.
#[derive(Default)]
pub struct ClaudeStreamParser {
    pub session_id: Option<String>,
    pub final_text: Option<String>,
    pub error: Option<String>,
    delta_buf: String,
    tool_names: HashMap<String, String>,
}

impl ClaudeStreamParser {
    pub fn take_final_text(&mut self) -> String {
        self.final_text
            .take()
            .unwrap_or_else(|| std::mem::take(&mut self.delta_buf))
    }

    pub fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let agent = AgentKind::Claude;
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
            Some("system") => {
                if value.get("subtype").and_then(Value::as_str) == Some("init") {
                    if let Some(id) = value.get("session_id").and_then(Value::as_str) {
                        self.session_id = Some(id.to_owned());
                        out.push(AgentEvent::SessionStarted {
                            agent,
                            session_id: id.to_owned(),
                        });
                    }
                }
            }
            Some("stream_event") => {
                let event = value.get("event").cloned().unwrap_or(Value::Null);
                // Only surface top-level assistant text (not subagent output).
                let top_level = value
                    .get("parent_tool_use_id")
                    .map(Value::is_null)
                    .unwrap_or(true);
                if top_level
                    && event.get("type").and_then(Value::as_str) == Some("content_block_delta")
                {
                    if let Some(delta) = event.get("delta") {
                        if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
                            if let Some(text) = delta.get("text").and_then(Value::as_str) {
                                self.delta_buf.push_str(text);
                                out.push(AgentEvent::TextDelta {
                                    agent,
                                    text: text.to_owned(),
                                });
                            }
                        }
                        // thinking_delta / signature_delta intentionally ignored:
                        // hidden reasoning is never exposed.
                    }
                }
            }
            Some("assistant") => {
                let content = value
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for block in content {
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_use") => {
                            let name = block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_owned();
                            if let Some(id) = block.get("id").and_then(Value::as_str) {
                                if self
                                    .tool_names
                                    .insert(id.to_owned(), name.clone())
                                    .is_some()
                                {
                                    continue; // snapshot repeats; already reported
                                }
                            }
                            let detail = tool_detail(&name, block.get("input"));
                            out.push(AgentEvent::ToolStarted {
                                agent,
                                name,
                                detail,
                            });
                        }
                        Some("text") => {
                            // Assistant text snapshots duplicate the deltas;
                            // deltas already carried the content.
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                if let Some(content) = value.pointer("/message/content").and_then(Value::as_array) {
                    for block in content {
                        if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                            if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                                if let Some(name) = self.tool_names.remove(id) {
                                    out.push(AgentEvent::ToolFinished { agent, name });
                                }
                            }
                        }
                    }
                }
            }
            Some("result") => {
                let is_error = value
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(id) = value.get("session_id").and_then(Value::as_str) {
                    self.session_id.get_or_insert_with(|| id.to_owned());
                }
                let text = value.get("result").and_then(Value::as_str).unwrap_or("");
                if is_error {
                    self.error = Some(if text.is_empty() {
                        "claude reported an error".to_owned()
                    } else {
                        text.to_owned()
                    });
                } else if !text.is_empty() {
                    self.final_text = Some(text.to_owned());
                }
                if let Some(usage) = value.get("usage") {
                    out.push(AgentEvent::Usage {
                        agent,
                        input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
                        output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
                    });
                }
            }
            Some(_) | None => {
                // Unknown top-level type: tolerated by design.
            }
        }
        out
    }
}

/// Human-readable one-line detail for a tool invocation, used for activity
/// display ("Reading src/auth/session.ts"). Never includes file contents.
fn tool_detail(name: &str, input: Option<&Value>) -> Option<String> {
    let input = input?;
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| input.get(k).and_then(Value::as_str))
            .map(str::to_owned)
    };
    let detail = match name {
        "Read" | "Write" | "Edit" | "NotebookEdit" => pick(&["file_path", "path"]),
        "Bash" => pick(&["description", "command"]),
        "Grep" => pick(&["pattern"]),
        "Glob" => pick(&["pattern"]),
        "WebFetch" | "WebSearch" => pick(&["url", "query"]),
        "Task" | "Agent" => pick(&["description"]),
        _ => pick(&[
            "description",
            "file_path",
            "path",
            "pattern",
            "command",
            "query",
        ]),
    }?;
    const MAX: usize = 80;
    Some(if detail.chars().count() > MAX {
        let truncated: String = detail.chars().take(MAX - 1).collect();
        format!("{truncated}…")
    } else {
        detail
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_id_from_init() {
        let mut p = ClaudeStreamParser::default();
        let events = p.parse_line(
            r#"{"type":"system","subtype":"init","cwd":"/x","session_id":"abc-123","tools":[]}"#,
        );
        assert_eq!(
            events,
            vec![AgentEvent::SessionStarted {
                agent: AgentKind::Claude,
                session_id: "abc-123".into()
            }]
        );
        assert_eq!(p.session_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn parses_text_deltas_and_final_result() {
        let mut p = ClaudeStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hel"}},"parent_tool_use_id":null}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::TextDelta {
                agent: AgentKind::Claude,
                text: "Hel".into()
            }]
        );
        p.parse_line(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Hello","session_id":"s1","usage":{"input_tokens":10,"output_tokens":2}}"#,
        );
        assert_eq!(p.take_final_text(), "Hello");
    }

    #[test]
    fn thinking_deltas_are_never_surfaced() {
        let mut p = ClaudeStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"secret"}},"parent_tool_use_id":null}"#,
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn subagent_text_is_not_surfaced() {
        let mut p = ClaudeStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"sub"}},"parent_tool_use_id":"toolu_1"}"#,
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn parses_tool_start_and_finish() {
        let mut p = ClaudeStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"src/db.ts"}}]}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::ToolStarted {
                agent: AgentKind::Claude,
                name: "Read".into(),
                detail: Some("src/db.ts".into())
            }]
        );
        let ev = p.parse_line(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"..."}]}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::ToolFinished {
                agent: AgentKind::Claude,
                name: "Read".into()
            }]
        );
    }

    #[test]
    fn repeated_tool_snapshot_reports_once() {
        let mut p = ClaudeStreamParser::default();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
        assert_eq!(p.parse_line(line).len(), 1);
        assert_eq!(p.parse_line(line).len(), 0);
    }

    #[test]
    fn error_result_sets_error() {
        let mut p = ClaudeStreamParser::default();
        p.parse_line(
            r#"{"type":"result","subtype":"error","is_error":true,"result":"rate limited"}"#,
        );
        assert_eq!(p.error.as_deref(), Some("rate limited"));
    }

    #[test]
    fn unknown_event_types_are_ignored() {
        let mut p = ClaudeStreamParser::default();
        assert!(p
            .parse_line(r#"{"type":"totally_new_event","payload":{"x":1}}"#)
            .is_empty());
        assert!(p
            .parse_line(r#"{"type":"rate_limit_event","rate_limit_info":{}}"#)
            .is_empty());
    }

    #[test]
    fn malformed_line_yields_warning_not_panic() {
        let mut p = ClaudeStreamParser::default();
        let ev = p.parse_line("this is not json {");
        assert!(matches!(ev.as_slice(), [AgentEvent::ParserWarning { .. }]));
    }

    #[test]
    fn falls_back_to_delta_buffer_without_result_line() {
        let mut p = ClaudeStreamParser::default();
        p.parse_line(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"partial"}},"parent_tool_use_id":null}"#,
        );
        assert_eq!(p.take_final_text(), "partial");
    }
}
