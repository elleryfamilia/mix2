//! Adapter for the Claude Code CLI: a descriptor plus pure builders and a
//! decoder. All process handling lives in the shared runner.
//!
//! Invocation shape (verified against claude 2.1.x):
//!   claude -p --output-format stream-json --verbose --include-partial-messages \
//!          --append-system-prompt <role instructions> [--resume <session-id>]
//! with the prompt on stdin. `--append-system-prompt` layers mix2's role
//! instructions on top of Claude Code's own system prompt instead of
//! replacing it, and `--allowedTools` permits exactly the consult helper
//! and scratchpad-scoped writes (`.mix2/**`) without widening anything
//! else.

use super::agent::AgentRequest;
use super::descriptor::{
    AuthProbe, Capabilities, CapabilityLevel, DecodeOutcome, Decoder, Descriptor,
};
use super::{AgentEvent, AgentRole, HarnessKind};
use serde_json::Value;
use std::collections::HashMap;

pub static DESCRIPTOR: Descriptor = Descriptor {
    harness: HarnessKind::Claude,
    label: "claude",
    default_command: "claude",
    aliases: &[],
    command_env_override: "MIX2_CLAUDE_CMD",
    install_hint:
        "install Claude Code from https://claude.com/claude-code, then run `claude` once to sign in",
    login_hint: "run `claude` once (or `claude auth login`) to sign in",
    selection_note: None,
    prompt_in_args: false,
    capabilities: Capabilities {
        // Teammate consultations add no permission flags: writes are blocked
        // only by Claude Code's non-interactive default deny, which the
        // user's own permission config can widen.
        teammate_read_only: CapabilityLevel::Unverified,
        // Leads get exactly the consult helper plus `.mix2/**` writes via
        // --allowedTools — mechanically scoped.
        lead_permission_scoping: CapabilityLevel::Enforced,
        instruction_injection: CapabilityLevel::Enforced,
    },
    // The documented `--model` aliases (each resolves to the latest in its
    // family), plus explicit latest names.
    known_models: &["fable", "opus", "sonnet", "haiku"],
    version_args: &["--version"],
    parse_version,
    // `claude auth status` prints JSON with a `loggedIn` field.
    auth_probe: AuthProbe::JsonLoggedIn {
        args: &["auth", "status"],
    },
    build_args,
    new_decoder,
};

fn new_decoder() -> Box<dyn Decoder> {
    Box::new(ClaudeStreamParser::default())
}

/// Loadout-style shell hooks can prepend banner lines; take the line that
/// actually looks like a version.
fn parse_version(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .find(|l| l.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .or_else(|| raw.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("unknown")
        .to_owned()
}

fn build_args(request: &AgentRequest, resume: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        "--append-system-prompt".into(),
        request.instructions.clone(),
    ];
    if let Some(model) = &request.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    if request.role == AgentRole::Lead {
        // Targeted permissions only: the consult helper, plus writes
        // scoped to the team scratchpad (`.mix2/`) so the lead can leave
        // plans and notes without gaining any access to project files.
        // Everything else follows the user's own Claude Code permission
        // configuration.
        args.push("--allowedTools".into());
        args.push("Bash(mix2-consult:*)".into());
        args.push("Write(.mix2/**)".into());
        args.push("Edit(.mix2/**)".into());
    }
    if let Some(id) = resume {
        args.push("--resume".into());
        args.push(id.into());
    }
    args
}

/// Tolerant parser for `claude --output-format stream-json` lines.
/// Unknown event types are ignored (with a parser warning in debug logs);
/// they must never crash mix2. Thinking deltas are consumed for state but
/// never surfaced as text.
#[derive(Default)]
pub struct ClaudeStreamParser {
    pub session_id: Option<String>,
    pub final_text: Option<String>,
    pub error: Option<String>,
    delta_buf: String,
    tool_names: HashMap<String, String>,
    model_reported: bool,
}

impl Decoder for ClaudeStreamParser {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        ClaudeStreamParser::parse_line(self, line)
    }

    fn finish(&mut self) -> DecodeOutcome {
        DecodeOutcome {
            error: self.error.take(),
            final_text: self.final_text.take(),
            // The stream can end without a result line; the accumulated
            // deltas are the best remaining answer.
            fallback_text: std::mem::take(&mut self.delta_buf),
            session_id: self.session_id.clone(),
        }
    }
}

impl ClaudeStreamParser {
    pub fn take_final_text(&mut self) -> String {
        self.final_text
            .take()
            .unwrap_or_else(|| std::mem::take(&mut self.delta_buf))
    }

    pub fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let line = line.trim();
        if line.is_empty() {
            return vec![];
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                return vec![AgentEvent::ParserWarning {
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
                            session_id: id.to_owned(),
                        });
                    }
                }
            }
            Some("stream_event") => {
                let event = value.get("event").cloned().unwrap_or(Value::Null);
                if !self.model_reported
                    && event.get("type").and_then(Value::as_str) == Some("message_start")
                {
                    if let Some(model) = event.pointer("/message/model").and_then(Value::as_str) {
                        self.model_reported = true;
                        out.push(AgentEvent::ModelObserved {
                            model: model.to_owned(),
                        });
                    }
                }
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
                            out.push(AgentEvent::ToolStarted { name, detail });
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
                                    out.push(AgentEvent::ToolFinished { name });
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
    use crate::agents::agent::AgentRequest;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn request(role: AgentRole, model: Option<&str>) -> AgentRequest {
        AgentRequest {
            prompt: "the prompt".into(),
            cwd: PathBuf::from("/repo"),
            role,
            turn_id: Uuid::nil(),
            model: model.map(str::to_owned),
            instructions: "ROLE\nsays \"hi\"".into(),
            env: HashMap::new(),
            path_prepend: None,
            runtime_dir: None,
        }
    }

    #[test]
    fn build_args_lead_start_golden() {
        assert_eq!(
            build_args(&request(AgentRole::Lead, Some("sonnet")), None),
            vec![
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--append-system-prompt",
                "ROLE\nsays \"hi\"", // verbatim: one argv element, no quoting
                "--model",
                "sonnet",
                "--allowedTools",
                "Bash(mix2-consult:*)",
                "Write(.mix2/**)",
                "Edit(.mix2/**)",
            ]
        );
    }

    #[test]
    fn build_args_teammate_resume_golden() {
        // Teammates get no extra permissions; resume appends last.
        assert_eq!(
            build_args(&request(AgentRole::Teammate, None), Some("sess-1")),
            vec![
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--append-system-prompt",
                "ROLE\nsays \"hi\"",
                "--resume",
                "sess-1",
            ]
        );
    }

    #[test]
    fn version_parser_skips_banner_lines() {
        assert_eq!(
            parse_version("loadout ready\n2.1.232 (Claude Code)\n"),
            "2.1.232 (Claude Code)"
        );
        assert_eq!(parse_version("\nno digits here\n"), "no digits here");
        assert_eq!(parse_version(""), "unknown");
    }

    #[test]
    fn decoder_finish_prefers_result_and_falls_back_to_deltas() {
        let mut p = ClaudeStreamParser::default();
        Decoder::parse_line(
            &mut p,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"partial"}},"parent_tool_use_id":null}"#,
        );
        let outcome = Decoder::finish(&mut p);
        assert_eq!(outcome.final_text, None);
        assert_eq!(outcome.fallback_text, "partial");

        let mut p = ClaudeStreamParser::default();
        p.parse_line(r#"{"type":"result","subtype":"success","is_error":false,"result":"Hello","session_id":"s1"}"#);
        let outcome = Decoder::finish(&mut p);
        assert_eq!(outcome.final_text.as_deref(), Some("Hello"));
        assert_eq!(outcome.session_id.as_deref(), Some("s1"));
        assert!(outcome.error.is_none());
    }

    #[test]
    fn parses_session_id_from_init() {
        let mut p = ClaudeStreamParser::default();
        let events = p.parse_line(
            r#"{"type":"system","subtype":"init","cwd":"/x","session_id":"abc-123","tools":[]}"#,
        );
        assert_eq!(
            events,
            vec![AgentEvent::SessionStarted {
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
        assert_eq!(ev, vec![AgentEvent::TextDelta { text: "Hel".into() }]);
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
        let outcome = Decoder::finish(&mut p);
        assert_eq!(outcome.error.as_deref(), Some("rate limited"));
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
