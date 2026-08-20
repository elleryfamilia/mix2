//! Adapter for the OpenAI Codex CLI: a descriptor plus pure builders and a
//! decoder. All process handling lives in the shared runner.
//!
//! Invocation shape (verified against codex-cli 0.146.x):
//!   codex exec --json --skip-git-repo-check [-c developer_instructions=...] -
//!   codex exec resume <thread-id> --json --skip-git-repo-check ... -
//! with the prompt on stdin and the working directory set on the child
//! process. `-c developer_instructions=...` layers mix2's role instructions
//! on top of Codex's built-in agent instructions per run, without touching
//! the user's `~/.codex/config.toml`.
//!
//! Sandbox note: `codex exec` defaults to a read-only sandbox that blocks
//! both Unix-socket connects and all file writes (verified empirically), so
//! a Codex *lead* could never reach the consult helper. Leads therefore run
//! with `sandbox_mode="workspace-write"` — Codex's standard interactive
//! sandbox level (workspace-writable, still no network) — plus the mix2
//! runtime dir added to writable roots for consult file IPC. Teammate
//! consultations keep the user's default sandbox untouched.

use super::agent::AgentRequest;
use super::descriptor::{
    AuthProbe, Capabilities, CapabilityLevel, DecodeOutcome, Decoder, Descriptor,
};
use super::{AgentEvent, AgentRole, HarnessKind};
use serde_json::Value;
use std::collections::HashMap;

pub static DESCRIPTOR: Descriptor = Descriptor {
    harness: HarnessKind::Codex,
    label: "codex",
    default_command: "codex",
    aliases: &[],
    command_env_override: "MIX2_CODEX_CMD",
    install_hint: "install Codex from https://developers.openai.com/codex/cli (`npm i -g @openai/codex`), then run `codex login`",
    login_hint: "run `codex login`",
    selection_note: None,
    prompt_in_args: false,
    capabilities: Capabilities {
        // Teammate consultations run in codex exec's default read-only
        // sandbox, which blocks all file writes — verified empirically.
        teammate_read_only: CapabilityLevel::Enforced,
        // Leads run workspace-write for consult IPC, so the sandbox allows
        // project writes; the `.mix2/`-only rule is instruction-enforced.
        lead_permission_scoping: CapabilityLevel::Unverified,
        instruction_injection: CapabilityLevel::Enforced,
    },
    // Curated: the models codex's `-m` commonly accepts. Replace with a
    // provider listing when the CLI grows one.
    known_models: &["gpt-5.3-codex", "gpt-5-codex", "gpt-5", "gpt-5-codex-mini"],
    models_args: None,
    version_args: &["--version"],
    parse_version,
    // `codex login status` exits 0 when signed in, non-zero otherwise.
    auth_probe: AuthProbe::ExitStatus {
        args: &["login", "status"],
    },
    build_args,
    new_decoder,
};

fn new_decoder() -> Box<dyn Decoder> {
    Box::new(CodexStreamParser::default())
}

fn parse_version(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

fn build_args(request: &AgentRequest, resume: Option<&str>) -> Vec<String> {
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

impl Decoder for CodexStreamParser {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        CodexStreamParser::parse_line(self, line)
    }

    fn finish(&mut self) -> DecodeOutcome {
        DecodeOutcome {
            error: self.error.take(),
            final_text: self.final_text.take(),
            fallback_text: String::new(),
            session_id: self.thread_id.clone(),
        }
    }
}

impl CodexStreamParser {
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
            Some("thread.started") => {
                if let Some(id) = value.get("thread_id").and_then(Value::as_str) {
                    self.thread_id = Some(id.to_owned());
                    out.push(AgentEvent::SessionStarted {
                        session_id: id.to_owned(),
                    });
                }
            }
            Some("turn.started") => {}
            Some("turn.completed") => {
                if let Some(usage) = value.get("usage") {
                    out.push(AgentEvent::Usage {
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
                                    text: delta.to_owned(),
                                });
                            }
                            _ => {}
                        }
                        *seen = text.len();
                        if t == "item.completed" {
                            self.final_text = Some(text.to_owned());
                            out.push(AgentEvent::Message {
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
                                name: "shell".into(),
                            });
                        } else if let std::collections::hash_map::Entry::Vacant(entry) =
                            self.running_commands.entry(item_id)
                        {
                            entry.insert(command.clone());
                            out.push(AgentEvent::ToolStarted {
                                name: "shell".into(),
                                detail: Some(short_detail(&command)),
                            });
                        }
                    }
                    Some("file_change") => {
                        if t == "item.completed" {
                            out.push(AgentEvent::ToolFinished {
                                name: "edit".into(),
                            });
                        } else {
                            let detail = item
                                .get("changes")
                                .and_then(Value::as_array)
                                .map(|c| format!("{} file(s)", c.len()));
                            out.push(AgentEvent::ToolStarted {
                                name: "edit".into(),
                                detail,
                            });
                        }
                    }
                    Some("web_search") => {
                        if t == "item.completed" {
                            out.push(AgentEvent::ToolFinished {
                                name: "search".into(),
                            });
                        } else {
                            out.push(AgentEvent::ToolStarted {
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
    use crate::agents::agent::AgentRequest;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn request(
        role: AgentRole,
        model: Option<&str>,
        instructions: &str,
        runtime_dir: Option<&str>,
    ) -> AgentRequest {
        AgentRequest {
            prompt: "the prompt".into(),
            cwd: PathBuf::from("/repo"),
            role,
            turn_id: Uuid::nil(),
            model: model.map(str::to_owned),
            instructions: instructions.into(),
            env: HashMap::new(),
            path_prepend: None,
            runtime_dir: runtime_dir.map(PathBuf::from),
        }
    }

    #[test]
    fn build_args_lead_start_golden() {
        // Instructions with a newline and quote pin the TOML quoting.
        assert_eq!(
            build_args(
                &request(
                    AgentRole::Lead,
                    Some("gpt-5-codex"),
                    "ROLE\nsays \"hi\"",
                    Some("/tmp/mix2/s1")
                ),
                None
            ),
            vec![
                "exec",
                "--json",
                "--skip-git-repo-check",
                "-c",
                "model=\"gpt-5-codex\"",
                "-c",
                "developer_instructions=\"ROLE\\nsays \\\"hi\\\"\"",
                "-c",
                "sandbox_mode=\"workspace-write\"",
                "-c",
                "sandbox_workspace_write.writable_roots=[\"/tmp/mix2/s1\"]",
                "-",
            ]
        );
    }

    #[test]
    fn build_args_teammate_keeps_default_sandbox() {
        // No sandbox override, no writable roots: the read-only default is
        // the teammate's enforcement.
        assert_eq!(
            build_args(&request(AgentRole::Teammate, None, "ROLE", None), None),
            vec![
                "exec",
                "--json",
                "--skip-git-repo-check",
                "-c",
                "developer_instructions=\"ROLE\"",
                "-",
            ]
        );
    }

    #[test]
    fn build_args_resume_golden() {
        // `resume <id>` comes immediately after `exec`, before the flags.
        assert_eq!(
            build_args(&request(AgentRole::Lead, None, "", None), Some("01a-b")),
            vec![
                "exec",
                "resume",
                "01a-b",
                "--json",
                "--skip-git-repo-check",
                "-c",
                "sandbox_mode=\"workspace-write\"",
                "-",
            ]
        );
    }

    #[test]
    fn version_parser_takes_first_nonempty_line() {
        assert_eq!(parse_version("\ncodex-cli 0.146.0\n"), "codex-cli 0.146.0");
        assert_eq!(parse_version(""), "unknown");
    }

    #[test]
    fn decoder_finish_carries_thread_and_error() {
        let mut p = CodexStreamParser::default();
        p.parse_line(r#"{"type":"thread.started","thread_id":"01a"}"#);
        p.parse_line(
            r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"OK"}}"#,
        );
        let outcome = Decoder::finish(&mut p);
        assert_eq!(outcome.final_text.as_deref(), Some("OK"));
        assert_eq!(outcome.session_id.as_deref(), Some("01a"));
        assert_eq!(outcome.fallback_text, "");

        let mut p = CodexStreamParser::default();
        p.parse_line(r#"{"type":"turn.failed","error":{"message":"rate limited"}}"#);
        assert_eq!(
            Decoder::finish(&mut p).error.as_deref(),
            Some("rate limited")
        );
    }

    #[test]
    fn parses_thread_id() {
        let mut p = CodexStreamParser::default();
        let ev = p.parse_line(r#"{"type":"thread.started","thread_id":"01a00224-aaaa"}"#);
        assert_eq!(
            ev,
            vec![AgentEvent::SessionStarted {
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
        assert!(ev.contains(&AgentEvent::Message { text: "OK".into() }));
        assert_eq!(p.final_text.as_deref(), Some("OK"));
    }

    #[test]
    fn incremental_agent_message_produces_deltas() {
        let mut p = CodexStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"item.updated","item":{"id":"m1","type":"agent_message","text":"Hel"}}"#,
        );
        assert_eq!(ev, vec![AgentEvent::TextDelta { text: "Hel".into() }]);
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
