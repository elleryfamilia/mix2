//! Adapter for the OpenCode CLI: a descriptor plus pure builders and a
//! decoder. All process handling lives in the shared runner.
//!
//! Invocation shape (verified against opencode 1.16.2 on this machine; the
//! stream was pinned with a live tool-using probe against a free model, so
//! no quota was spent):
//!   opencode run --format json --agent plan [-m provider/model] \
//!                [-s <sessionID>] "<prompt>"
//! The message is positional and there is no separate instruction channel —
//! mix2's role instructions ride in-band ahead of the prompt.
//!
//! Event shape: JSONL envelopes `{type, timestamp, sessionID, part:{...}}`:
//! - `step_start`                       — turn segment opens
//! - `tool_use`  (part.type "tool")     — one tool invocation; part.state
//!   carries status ("running"/"completed"), input, and a display title
//! - `text`      (part.type "text")     — a complete text chunk (not a
//!   character delta); the final answer is their concatenation
//! - `step_finish` (part.type "step-finish") — reason ("tool-calls"/"stop"),
//!   tokens {input, output, reasoning, cache{read,write}}, cost
//!
//! Every envelope carries the sessionID used for `-s` resumes.
//!
//! Teammate-only for now: the built-in `plan` agent mechanically denies
//! project edits (`edit: deny *`, verified via `opencode agent list`; its
//! one exception is markdown under `.opencode/plans/` — OpenCode's own
//! planning scratch area). No verified mechanism scopes a *lead*'s writes
//! to `.mix2/` or reaches the consult helper.
//!
//! Auth: `opencode auth list` prints a credential inventory (exit 0). An
//! inventory proves configuration, not live validity → `Configured`, which
//! never blocks startup.
//!
//! Models: `opencode models` enumerates `provider/model` ids (dozens). The
//! descriptor declares the listing command; the runner fetches it bounded
//! and cached at startup, and an enumeration failure just falls back to
//! typed model entry — it never marks the harness unavailable.

use super::agent::AgentRequest;
use super::descriptor::{
    AuthProbe, Capabilities, CapabilityLevel, DecodeOutcome, Decoder, Descriptor,
};
use super::{AgentEvent, HarnessKind};
use serde_json::Value;

pub static DESCRIPTOR: Descriptor = Descriptor {
    harness: HarnessKind::Opencode,
    label: "opencode",
    default_command: "opencode",
    aliases: &[],
    command_env_override: "MIX2_OPENCODE_CMD",
    install_hint: "install OpenCode from https://opencode.ai (`curl -fsSL https://opencode.ai/install | bash`), then run `opencode auth login`",
    login_hint: "run `opencode auth login`",
    selection_note: None,
    prompt_in_args: true,
    capabilities: Capabilities {
        // The built-in `plan` agent denies project edits mechanically
        // (verified `edit: deny *`; sole exception: markdown under
        // `.opencode/plans/`, OpenCode's own planning scratch area).
        teammate_read_only: CapabilityLevel::Enforced,
        // No verified way to scope a lead's writes to `.mix2/` or reach
        // the consult helper — teammate-only until that exists.
        lead_permission_scoping: CapabilityLevel::Unsupported,
        // Role instructions ride in-band ahead of the prompt.
        instruction_injection: CapabilityLevel::Unverified,
    },
    // Teammate-only natively, but leadable under the OS sandbox.
    sandboxable_lead: true,
    state_dirs: &[
        "~/.local/share/opencode",
        "~/.config/opencode",
        "~/.cache/opencode",
        "~/.local/state/opencode",
    ],
    credential_files: &["~/.local/share/opencode/auth.json"],
    env_keep_sandboxed: &[],
    // Live enumeration via `models_args` supplies the real list; this
    // static fallback stays empty so a failed enumeration degrades to
    // typed `/model two provider/model` entry.
    known_models: &[],
    models_args: Some(&["models"]),
    version_args: &["--version"],
    parse_version,
    // `opencode auth list` inventories stored credentials; presence means
    // Configured (unverified validity), never Authenticated.
    auth_probe: AuthProbe::CredentialInventory {
        args: &["auth", "list"],
    },
    build_args,
    new_decoder,
};

fn new_decoder() -> Box<dyn Decoder> {
    Box::new(OpencodeStreamParser::default())
}

fn parse_version(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

fn build_args(request: &AgentRequest, resume: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["run".into(), "--format".into(), "json".into()];
    // The read-only planning agent keeps an OpenCode *teammate* from
    // editing. A sandboxed lead needs to write `.mix2/`, so it drops the
    // plan agent and relies on the OS sandbox instead. Keyed on the
    // resolved sandbox (not the role): unsandboxed argv is byte-identical
    // to today, and an unsandboxed OpenCode lead is refused upstream.
    if request.sandbox.is_none() {
        args.push("--agent".into());
        args.push("plan".into());
    }
    if let Some(model) = &request.model {
        args.push("-m".into());
        args.push(model.clone());
    }
    if let Some(id) = resume {
        args.push("-s".into());
        args.push(id.into());
    }
    if request.instructions.is_empty() {
        args.push(request.prompt.clone());
    } else {
        args.push(format!("{}\n\n{}", request.instructions, request.prompt));
    }
    args
}

/// Tolerant parser for `opencode run --format json` envelopes. Text parts
/// arrive as complete chunks; their concatenation is the final answer.
/// Tool parts map to started/finished events; usage comes from the last
/// step_finish. Unknown part and envelope types must never panic.
#[derive(Default)]
pub struct OpencodeStreamParser {
    pub session_id: Option<String>,
    pub error: Option<String>,
    text: String,
    usage: Option<(Option<u64>, Option<u64>)>,
}

impl Decoder for OpencodeStreamParser {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        OpencodeStreamParser::parse_line(self, line)
    }

    fn finish(&mut self) -> DecodeOutcome {
        let text = std::mem::take(&mut self.text);
        DecodeOutcome {
            error: self.error.take(),
            final_text: if text.is_empty() { None } else { Some(text) },
            fallback_text: String::new(),
            session_id: self.session_id.clone(),
        }
    }
}

impl OpencodeStreamParser {
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
        if self.session_id.is_none() {
            if let Some(id) = value.get("sessionID").and_then(Value::as_str) {
                self.session_id = Some(id.to_owned());
                out.push(AgentEvent::SessionStarted {
                    session_id: id.to_owned(),
                });
            }
        }

        let part = value.get("part").cloned().unwrap_or(Value::Null);
        match value.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        // Text parts are complete paragraphs; keep them
                        // readable when several arrive.
                        if !self.text.is_empty() {
                            self.text.push_str("\n\n");
                        }
                        self.text.push_str(text);
                        out.push(AgentEvent::TextDelta {
                            text: text.to_owned(),
                        });
                    }
                }
            }
            Some("tool_use") => {
                let name = part
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_owned();
                let status = part
                    .pointer("/state/status")
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                let detail = part
                    .pointer("/state/title")
                    .and_then(Value::as_str)
                    .map(short_detail);
                match status {
                    // A completed part is the whole lifecycle in one event.
                    "completed" => {
                        out.push(AgentEvent::ToolStarted {
                            name: name.clone(),
                            detail,
                        });
                        out.push(AgentEvent::ToolFinished { name });
                    }
                    "error" => {
                        out.push(AgentEvent::ToolFinished { name });
                    }
                    _ => {
                        out.push(AgentEvent::ToolStarted { name, detail });
                    }
                }
            }
            Some("step_finish") => {
                let tokens = part.get("tokens").cloned().unwrap_or(Value::Null);
                self.usage = Some((
                    tokens.get("input").and_then(Value::as_u64),
                    tokens.get("output").and_then(Value::as_u64),
                ));
                if part.get("reason").and_then(Value::as_str) == Some("stop") {
                    if let Some((input_tokens, output_tokens)) = self.usage {
                        out.push(AgentEvent::Usage {
                            input_tokens,
                            output_tokens,
                        });
                    }
                }
            }
            Some("error") => {
                let message = value
                    .pointer("/part/error")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("message").and_then(Value::as_str))
                    .unwrap_or("opencode reported an error")
                    .to_owned();
                self.error = Some(message);
            }
            Some("step_start") | Some(_) | None => {
                // step_start and unknown envelopes carry nothing to surface.
            }
        }
        out
    }
}

fn short_detail(title: &str) -> String {
    const MAX: usize = 80;
    if title.chars().count() > MAX {
        let truncated: String = title.chars().take(MAX - 1).collect();
        format!("{truncated}…")
    } else {
        title.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::agent::AgentRequest;
    use crate::agents::AgentRole;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn request(model: Option<&str>, instructions: &str) -> AgentRequest {
        AgentRequest {
            prompt: "evaluate the cache".into(),
            cwd: PathBuf::from("/repo"),
            role: AgentRole::Teammate,
            turn_id: Uuid::nil(),
            model: model.map(str::to_owned),
            instructions: instructions.into(),
            env: HashMap::new(),
            path_prepend: None,
            runtime_dir: None,
            sandbox: None,
        }
    }

    fn sandboxed(mut request: AgentRequest) -> AgentRequest {
        request.role = AgentRole::Lead;
        request.sandbox = Some(crate::sandbox::SandboxSpec {
            engine: crate::sandbox::SandboxEngine::Seatbelt,
            policy: crate::sandbox::SandboxPolicy::with_writable(vec![]),
            env_remove: Vec::new(),
        });
        request
    }

    #[test]
    fn build_args_teammate_golden() {
        assert_eq!(
            build_args(&request(Some("anthropic/claude-sonnet-4-5"), "ROLE"), None),
            vec![
                "run",
                "--format",
                "json",
                "--agent",
                "plan",
                "-m",
                "anthropic/claude-sonnet-4-5",
                "ROLE\n\nevaluate the cache",
            ]
        );
    }

    #[test]
    fn build_args_drops_plan_agent_for_a_sandboxed_lead() {
        // The sandbox scopes writes, so an OpenCode lead runs the default
        // (writing) agent rather than the read-only plan agent.
        let args = build_args(&sandboxed(request(None, "ROLE")), None);
        assert!(!args.contains(&"--agent".to_owned()));
        assert!(!args.contains(&"plan".to_owned()));
        assert!(args.contains(&"run".to_owned()));
    }

    #[test]
    fn build_args_resume_golden() {
        assert_eq!(
            build_args(&request(None, ""), Some("ses_1")),
            vec![
                "run",
                "--format",
                "json",
                "--agent",
                "plan",
                "-s",
                "ses_1",
                "evaluate the cache",
            ]
        );
    }

    #[test]
    fn session_id_comes_from_the_first_envelope() {
        let mut p = OpencodeStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"step_start","timestamp":1,"sessionID":"ses_9","part":{"type":"step-start"}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::SessionStarted {
                session_id: "ses_9".into()
            }]
        );
        // Later envelopes do not re-announce it.
        assert!(p
            .parse_line(r#"{"type":"step_start","sessionID":"ses_9","part":{}}"#)
            .is_empty());
    }

    #[test]
    fn text_parts_accumulate_into_the_final_answer() {
        let mut p = OpencodeStreamParser::default();
        p.parse_line(r#"{"type":"text","sessionID":"s","part":{"type":"text","text":"first"}}"#);
        let ev = p.parse_line(
            r#"{"type":"text","sessionID":"s","part":{"type":"text","text":"second"}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::TextDelta {
                text: "second".into()
            }]
        );
        let outcome = Decoder::finish(&mut p);
        assert_eq!(outcome.final_text.as_deref(), Some("first\n\nsecond"));
    }

    #[test]
    fn completed_tool_part_maps_to_started_and_finished() {
        let mut p = OpencodeStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"tool_use","sessionID":"s","part":{"type":"tool","tool":"read","state":{"status":"completed","title":"note.txt"}}}"#,
        );
        assert_eq!(
            ev,
            vec![
                AgentEvent::SessionStarted {
                    session_id: "s".into()
                },
                AgentEvent::ToolStarted {
                    name: "read".into(),
                    detail: Some("note.txt".into())
                },
                AgentEvent::ToolFinished {
                    name: "read".into()
                },
            ]
        );
    }

    #[test]
    fn usage_comes_from_the_stopping_step_finish() {
        let mut p = OpencodeStreamParser::default();
        assert!(p
            .parse_line(
                r#"{"type":"step_finish","sessionID":"s","part":{"type":"step-finish","reason":"tool-calls","tokens":{"input":100,"output":5}}}"#
            )
            .iter()
            .all(|e| !matches!(e, AgentEvent::Usage { .. })));
        let ev = p.parse_line(
            r#"{"type":"step_finish","sessionID":"s","part":{"type":"step-finish","reason":"stop","tokens":{"input":120,"output":9}}}"#,
        );
        assert!(ev.contains(&AgentEvent::Usage {
            input_tokens: Some(120),
            output_tokens: Some(9)
        }));
    }

    #[test]
    fn error_envelope_fails_the_run() {
        let mut p = OpencodeStreamParser::default();
        p.parse_line(r#"{"type":"error","sessionID":"s","part":{"error":"provider exploded"}}"#);
        assert_eq!(
            Decoder::finish(&mut p).error.as_deref(),
            Some("provider exploded")
        );
    }

    #[test]
    fn malformed_and_unknown_lines_are_tolerated() {
        let mut p = OpencodeStreamParser::default();
        assert!(matches!(
            p.parse_line("garbage{{{").as_slice(),
            [AgentEvent::ParserWarning { .. }]
        ));
        assert!(
            p.parse_line(r#"{"type":"future_part","sessionID":"s","part":{"type":"holo"}}"#)
                .len()
                <= 1
        );
    }

    #[test]
    fn opencode_is_teammate_only_by_capability() {
        assert!(!DESCRIPTOR.capabilities.lead_eligible());
        assert!(DESCRIPTOR.capabilities.teammate_eligible());
    }
}
