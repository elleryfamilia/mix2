//! Adapter for the Cursor Agent CLI (`cursor-agent`): a descriptor plus
//! pure builders and a decoder. All process handling lives in the shared
//! runner.
//!
//! Invocation shape (verified against cursor-agent 2026.08.04 on this
//! machine — help/status/models probes are quota-free):
//!   cursor-agent -p --output-format stream-json --stream-partial-output \
//!                --trust --mode plan [--model <m>] [--resume <chatId>] "<prompt>"
//! The prompt is a positional argument, not stdin, and there is no separate
//! instruction-injection flag — mix2's role instructions ride in-band ahead
//! of the prompt (hence `instruction_injection: Unverified`).
//!
//! Teammate-only for now: `--mode plan` is a real read-only mode (enforced
//! teammate capability), but no verified mechanism scopes a *lead*'s writes
//! to `.mix2/` or reaches the consult helper, so `lead_permission_scoping`
//! is `Unsupported` and the slot picker refuses Cursor as coordinator.
//!
//! `--trust` skips the workspace-trust prompt that would otherwise hang a
//! non-interactive run. It is disclosed as a selection note in the picker —
//! choosing Cursor (or configuring it in a slot) is the opt-in; the flag is
//! never passed on behalf of a team the user didn't pick.
//!
//! A *sandboxed lead* additionally runs with `--force`. Cursor's approval
//! prompts cannot fire in `-p` mode, so any command outside the user's own
//! allowlist is silently auto-rejected — observed live as `mix2-consult`
//! (and everything else) dying with `Rejected`, which breaks the consult
//! contract. `--force` flips that default to allow-unless-explicitly-denied:
//! the user's `deny` rules still win, and the OS sandbox — the mechanism
//! that makes a Cursor lead permissible at all — remains the write
//! confinement. Teammates never get `--force`; `--mode plan` is their
//! enforcement.
//!
//! Free-plan caveat (observed live): naming a model can be refused with a
//! BARE TEXT line mid-stream (`ActionRequiredError: Named models
//! unavailable...`). The decoder tolerates non-JSON lines and maps them to
//! a failure when no final result arrives; `auto` is the model fallback.

use super::agent::AgentRequest;
use super::descriptor::{
    AuthProbe, Capabilities, CapabilityLevel, DecodeOutcome, Decoder, Descriptor,
};
use super::{AgentEvent, AgentRole, HarnessKind};
use serde_json::Value;

pub static DESCRIPTOR: Descriptor = Descriptor {
    harness: HarnessKind::Cursor,
    label: "cursor",
    default_command: "cursor-agent",
    aliases: &["cursor-agent"],
    command_env_override: "MIX2_CURSOR_CMD",
    install_hint:
        "install the Cursor CLI from https://cursor.com/cli, then run `cursor-agent login`",
    login_hint: "run `cursor-agent login`",
    selection_note: Some(
        "runs with --trust: picking Cursor marks this workspace trusted; as coordinator it also \
         auto-approves its commands (--force) inside the OS sandbox",
    ),
    prompt_in_args: true,
    capabilities: Capabilities {
        // `--mode plan` is Cursor's own read-only/planning mode.
        teammate_read_only: CapabilityLevel::Enforced,
        // No verified way to scope a lead's writes to `.mix2/` or reach the
        // consult helper — teammate-only until that exists.
        lead_permission_scoping: CapabilityLevel::Unsupported,
        // Role instructions ride in-band ahead of the prompt; there is no
        // dedicated system-prompt flag to make this mechanical.
        instruction_injection: CapabilityLevel::Unverified,
    },
    // Teammate-only natively, but leadable under the OS sandbox.
    sandboxable_lead: true,
    state_dirs: &["~/.cursor"],
    credential_files: &["~/.cursor/cli-config.json"],
    env_keep_sandboxed: &[],
    // Curated from `cursor-agent --list-models` (large list; the picker's
    // filter handles the rest via manual entry). `auto` is the default and
    // the safe fallback on plans that restrict named models.
    known_models: &[
        "auto",
        "gpt-5.3-codex",
        "gpt-5.1",
        "claude-4.5-sonnet",
        "claude-4.5-sonnet-thinking",
        "gemini-3-flash",
    ],
    models_args: None,
    version_args: &["--version"],
    parse_version,
    // `cursor-agent status` exits 0 when signed in ("✓ Logged in as …").
    auth_probe: AuthProbe::ExitStatus { args: &["status"] },
    build_args,
    new_decoder,
};

fn new_decoder() -> Box<dyn Decoder> {
    Box::new(CursorStreamParser::default())
}

fn parse_version(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

fn build_args(request: &AgentRequest, resume: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--stream-partial-output".into(),
        "--trust".into(),
    ];
    // Read-only planning mode keeps a Cursor teammate from editing. Only a
    // sandboxed *lead* drops it (the lead must write `.mix2/`, and the OS
    // sandbox scopes it); a sandboxed teammate keeps plan mode on top of the
    // sandbox — defense in depth. With no sandbox the argv is byte-identical
    // to today (and an unsandboxed Cursor lead is refused upstream), so
    // `mode = off` is a true rollback.
    let sandboxed_lead = request.sandbox.is_some() && request.role == AgentRole::Lead;
    if !sandboxed_lead {
        args.push("--mode".into());
        args.push("plan".into());
    } else {
        // Approvals cannot prompt in `-p` mode: without --force, every
        // command outside the user's own allowlist is silently
        // auto-rejected — including `mix2-consult`, which breaks the
        // consult contract outright (observed live). The OS sandbox is the
        // real confinement here (writes scoped to `.mix2/` plus the
        // runtime dir), and --force still honors explicit `deny` rules in
        // the user's Cursor config. Disclosed in the selection note.
        args.push("--force".into());
    }
    if let Some(model) = &request.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    if let Some(id) = resume {
        args.push("--resume".into());
        args.push(id.into());
    }
    // Positional prompt, with the role instructions in-band ahead of it —
    // cursor-agent has no separate instruction channel.
    if request.instructions.is_empty() {
        args.push(request.prompt.clone());
    } else {
        args.push(format!("{}\n\n{}", request.instructions, request.prompt));
    }
    args
}

/// Tolerant parser for `cursor-agent --output-format stream-json` lines.
/// The shape is near-identical to Claude Code's stream: `system`/`init`
/// carries the session id and model, `assistant` envelopes carry text
/// chunks, `result` is authoritative. Thinking output is never surfaced.
/// Bare non-JSON lines are remembered: if the stream ends without a result,
/// the last one becomes the failure message (the free-plan restriction
/// arrives exactly that way).
#[derive(Default)]
pub struct CursorStreamParser {
    pub session_id: Option<String>,
    pub final_text: Option<String>,
    pub error: Option<String>,
    delta_buf: String,
    /// Last bare (non-JSON) line seen, e.g. `ActionRequiredError: …`.
    stray_line: Option<String>,
    model_reported: bool,
}

impl Decoder for CursorStreamParser {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        CursorStreamParser::parse_line(self, line)
    }

    fn finish(&mut self) -> DecodeOutcome {
        let error = self.error.take().or_else(|| {
            // A stream that never produced a result and left a bare line is
            // a refusal in prose (named-model restriction, similar).
            if self.final_text.is_none() {
                self.stray_line.take()
            } else {
                None
            }
        });
        DecodeOutcome {
            error,
            final_text: self.final_text.take(),
            fallback_text: std::mem::take(&mut self.delta_buf),
            session_id: self.session_id.clone(),
        }
    }
}

impl CursorStreamParser {
    pub fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let line = line.trim();
        if line.is_empty() {
            return vec![];
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                self.stray_line = Some(line.to_owned());
                return vec![AgentEvent::ParserWarning {
                    message: format!("non-JSON line: {}", truncate_for_log(line)),
                }];
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
                    if !self.model_reported {
                        if let Some(model) = value.get("model").and_then(Value::as_str) {
                            self.model_reported = true;
                            out.push(AgentEvent::ModelObserved {
                                model: model.to_owned(),
                            });
                        }
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
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                self.delta_buf.push_str(text);
                                out.push(AgentEvent::TextDelta {
                                    text: text.to_owned(),
                                });
                            }
                        }
                    }
                    // `thinking` blocks intentionally ignored: hidden
                    // reasoning is never exposed.
                }
            }
            // The echo of the submitted prompt and standalone thinking
            // deltas carry nothing the UI should surface.
            Some("user") | Some("thinking") => {}
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
                        "cursor reported an error".to_owned()
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
                // Unknown event types tolerated by design.
            }
        }
        out
    }
}

fn truncate_for_log(line: &str) -> String {
    const MAX: usize = 120;
    if line.chars().count() > MAX {
        let truncated: String = line.chars().take(MAX - 1).collect();
        format!("{truncated}…")
    } else {
        line.to_owned()
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
    fn build_args_drops_plan_mode_and_forces_approvals_for_a_sandboxed_lead() {
        // The sandbox enforces write scoping, so a Cursor lead runs without
        // `--mode plan` and can write — and with `--force`, because `-p`
        // mode auto-rejects anything outside the user's allowlist (which
        // silently killed `mix2-consult` in the field).
        let args = build_args(&sandboxed(request(Some("auto"), "ROLE")), None);
        assert!(!args.contains(&"--mode".to_owned()));
        assert!(!args.contains(&"plan".to_owned()));
        assert!(args.contains(&"--force".to_owned()));
        assert!(args.contains(&"--trust".to_owned()));
        assert!(args.contains(&"--model".to_owned()));
    }

    #[test]
    fn a_sandboxed_teammate_keeps_plan_mode_and_never_gets_force() {
        // Defense in depth: teammates stay read-only via `--mode plan` even
        // under the sandbox, and their approvals are never forced.
        let mut req = sandboxed(request(None, "ROLE"));
        req.role = AgentRole::Teammate;
        let args = build_args(&req, None);
        assert!(args.contains(&"--mode".to_owned()));
        assert!(args.contains(&"plan".to_owned()));
        assert!(!args.contains(&"--force".to_owned()));
    }

    #[test]
    fn build_args_teammate_golden() {
        assert_eq!(
            build_args(&request(Some("auto"), "ROLE"), None),
            vec![
                "-p",
                "--output-format",
                "stream-json",
                "--stream-partial-output",
                "--trust",
                "--mode",
                "plan",
                "--model",
                "auto",
                "ROLE\n\nevaluate the cache",
            ]
        );
    }

    #[test]
    fn build_args_resume_and_bare_prompt_golden() {
        assert_eq!(
            build_args(&request(None, ""), Some("chat-1")),
            vec![
                "-p",
                "--output-format",
                "stream-json",
                "--stream-partial-output",
                "--trust",
                "--mode",
                "plan",
                "--resume",
                "chat-1",
                "evaluate the cache",
            ]
        );
    }

    #[test]
    fn parses_init_with_session_and_model() {
        let mut p = CursorStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"system","subtype":"init","session_id":"chat-9","model":"auto","permissionMode":"plan"}"#,
        );
        assert_eq!(ev.len(), 2);
        assert!(ev.contains(&AgentEvent::SessionStarted {
            session_id: "chat-9".into()
        }));
        assert!(ev.contains(&AgentEvent::ModelObserved {
            model: "auto".into()
        }));
    }

    #[test]
    fn assistant_text_streams_and_result_is_authoritative() {
        let mut p = CursorStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hel"}]}}"#,
        );
        assert_eq!(ev, vec![AgentEvent::TextDelta { text: "Hel".into() }]);
        p.parse_line(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Hello","session_id":"chat-9","usage":{"input_tokens":9,"output_tokens":2}}"#,
        );
        let outcome = Decoder::finish(&mut p);
        assert_eq!(outcome.final_text.as_deref(), Some("Hello"));
        assert_eq!(outcome.session_id.as_deref(), Some("chat-9"));
        assert!(outcome.error.is_none());
    }

    #[test]
    fn thinking_is_never_surfaced() {
        let mut p = CursorStreamParser::default();
        assert!(p
            .parse_line(r#"{"type":"thinking","text":"secret"}"#)
            .is_empty());
        assert!(p
            .parse_line(
                r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"secret"}]}}"#
            )
            .is_empty());
    }

    #[test]
    fn bare_error_line_without_result_becomes_the_failure() {
        let mut p = CursorStreamParser::default();
        let ev = p.parse_line(
            "ActionRequiredError: Named models unavailable on your plan. Use --model auto.",
        );
        assert!(matches!(ev.as_slice(), [AgentEvent::ParserWarning { .. }]));
        let outcome = Decoder::finish(&mut p);
        assert!(outcome
            .error
            .as_deref()
            .unwrap()
            .contains("Named models unavailable"));
    }

    #[test]
    fn bare_noise_is_tolerated_when_a_result_arrives() {
        let mut p = CursorStreamParser::default();
        p.parse_line("some banner noise");
        p.parse_line(r#"{"type":"result","subtype":"success","is_error":false,"result":"OK"}"#);
        let outcome = Decoder::finish(&mut p);
        assert!(outcome.error.is_none());
        assert_eq!(outcome.final_text.as_deref(), Some("OK"));
    }

    #[test]
    fn error_result_wins_over_stray_lines() {
        let mut p = CursorStreamParser::default();
        p.parse_line("noise");
        p.parse_line(
            r#"{"type":"result","subtype":"error","is_error":true,"result":"rate limited"}"#,
        );
        assert_eq!(
            Decoder::finish(&mut p).error.as_deref(),
            Some("rate limited")
        );
    }

    #[test]
    fn unknown_types_are_ignored() {
        let mut p = CursorStreamParser::default();
        assert!(p
            .parse_line(r#"{"type":"tool_call","name":"read"}"#)
            .is_empty());
        assert!(p.parse_line(r#"{"type":"user","message":{}}"#).is_empty());
    }

    #[test]
    fn cursor_is_teammate_only_by_capability() {
        assert!(!DESCRIPTOR.capabilities.lead_eligible());
        assert!(DESCRIPTOR.capabilities.teammate_eligible());
    }
}
