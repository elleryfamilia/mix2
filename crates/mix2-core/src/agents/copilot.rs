//! Adapter for the GitHub Copilot CLI (`copilot`): a descriptor plus pure
//! builders and a decoder. All process handling lives in the shared runner.
//!
//! Invocation shape (flags verified against copilot 1.0.78 via quota-free
//! `--help`/`help permissions`; the event vocabulary was pinned by the
//! plan's live probe on this machine, 2026-08-20):
//!   copilot -p "<prompt>" --output-format json --no-color \
//!           --no-custom-instructions --disable-builtin-mcps \
//!           --allow-all-tools --deny-tool write --deny-tool shell \
//!           [--model <m>] [--resume=<sessionId>]
//! The prompt is the `-p` value (never stdin), with mix2's role
//! instructions in-band ahead of it.
//!
//! Read-only recipe: non-interactive mode needs `--allow-all-tools`, but
//! the CLI documents that *denial rules always take precedence over allow
//! rules, even --allow-all-tools* — so `--deny-tool write --deny-tool
//! shell` mechanically blocks file mutation and shell execution. What
//! keeps `teammate_read_only` at `Unverified` rather than `Enforced`: the
//! CLI auto-loads the user's personal MCP servers and skills from
//! `~/.copilot` (observed live during planning) and 1.0.78 has no global
//! off-switch for them, so a user-configured MCP tool could still mutate
//! state. `--disable-builtin-mcps` removes the built-in GitHub MCP server,
//! and the picker disclosure says the rest out loud.
//!
//! Event vocabulary (JSONL, one object per line): `session.*` bookkeeping
//! (ignored), `user.message`, `assistant.turn_start`,
//! `assistant.message_start`/`assistant.message_delta` (marked
//! `"ephemeral":true` — streaming display only), `assistant.message`
//! (authoritative content + model + toolRequests + outputTokens),
//! `assistant.reasoning` (never surfaced), `assistant.turn_end`, and a
//! final `result` carrying sessionId, exitCode, and usage. The decoder
//! extracts fields through several likely paths, tolerating shape drift.
//!
//! Auth: no quota-free status probe exists (`login` only) — the plan's
//! call is `Unsupported`, never trial prompts; run-time auth failures
//! surface through the stderr tail. Headless token precedence:
//! COPILOT_GITHUB_TOKEN > GH_TOKEN > GITHUB_TOKEN > stored credential.

use super::agent::AgentRequest;
use super::descriptor::{
    AuthProbe, Capabilities, CapabilityLevel, DecodeOutcome, Decoder, Descriptor,
};
use super::{AgentEvent, HarnessKind};
use serde_json::Value;

pub static DESCRIPTOR: Descriptor = Descriptor {
    harness: HarnessKind::Copilot,
    label: "copilot",
    default_command: "copilot",
    aliases: &[],
    command_env_override: "MIX2_COPILOT_CMD",
    install_hint: "install the GitHub Copilot CLI (`npm i -g @github/copilot`), then run `copilot login`",
    login_hint: "run `copilot login` (headless: set COPILOT_GITHUB_TOKEN, GH_TOKEN, or GITHUB_TOKEN)",
    selection_note: Some(
        "write and shell tools are denied (denials outrank --allow-all-tools), but your personal Copilot MCP servers and skills from ~/.copilot still load",
    ),
    prompt_in_args: true,
    capabilities: Capabilities {
        // write/shell are denied with documented precedence, but the
        // user's own ~/.copilot MCP servers auto-load with no global
        // off-switch in 1.0.78 — a configured MCP tool could mutate.
        teammate_read_only: CapabilityLevel::Unverified,
        // No verified way to scope a lead's writes to `.mix2/` or reach
        // the consult helper — teammate-only until that exists.
        lead_permission_scoping: CapabilityLevel::Unsupported,
        // Role instructions ride in-band ahead of the prompt.
        instruction_injection: CapabilityLevel::Unverified,
    },
    // Teammate-only natively, but leadable under the OS sandbox. Copilot
    // authenticates headlessly via GitHub tokens, so those env vars survive
    // the credential strip.
    sandboxable_lead: true,
    state_dirs: &["~/.copilot", "~/.config/github-copilot"],
    // Copilot authenticates through GitHub, so the GitHub CLI's credential
    // store is its own — readable when Copilot leads, denied to other leads.
    credential_files: &[
        "~/.config/gh",
        "~/.config/github-copilot/apps.json",
        "~/.config/github-copilot/hosts.json",
    ],
    env_keep_sandboxed: &["GH_TOKEN", "GITHUB_TOKEN", "COPILOT_GITHUB_TOKEN"],
    // `auto` lets Copilot route; named-model ids are account-dependent, so
    // typed /model entry covers the rest.
    known_models: &["auto"],
    models_args: None,
    version_args: &["--version"],
    parse_version,
    // No status subcommand exists; never burn quota probing.
    auth_probe: AuthProbe::None,
    build_args,
    new_decoder,
};

fn new_decoder() -> Box<dyn Decoder> {
    Box::new(CopilotStreamParser::default())
}

fn parse_version(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

fn build_args(request: &AgentRequest, resume: Option<&str>) -> Vec<String> {
    let prompt = if request.instructions.is_empty() {
        request.prompt.clone()
    } else {
        format!("{}\n\n{}", request.instructions, request.prompt)
    };
    let mut args: Vec<String> = vec![
        "-p".into(),
        prompt,
        "--output-format".into(),
        "json".into(),
        "--no-color".into(),
        // Session isolation: the user's AGENTS.md instructions and the
        // built-in GitHub MCP server stay out of mix2 consultations.
        "--no-custom-instructions".into(),
        "--disable-builtin-mcps".into(),
        // Non-interactive mode requires blanket tool approval.
        "--allow-all-tools".into(),
    ];
    // The write/shell denials are what keep a Copilot *teammate* read-only
    // (they outrank `--allow-all-tools` by documented precedence). A
    // sandboxed lead needs to write `.mix2/` and run `mix2-consult`, so it
    // drops the denials and relies on the OS sandbox for scoping. Keyed on
    // the resolved sandbox, never the role: with no sandbox the denials
    // stay, so the argv is byte-identical to today and `mode = off` is a
    // true rollback.
    if request.sandbox.is_none() {
        args.push("--deny-tool".into());
        args.push("write".into());
        args.push("--deny-tool".into());
        args.push("shell".into());
    }
    if let Some(model) = &request.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    if let Some(id) = resume {
        args.push(format!("--resume={id}"));
    }
    args
}

/// Tolerant parser for `copilot --output-format json` JSONL. Ephemeral
/// deltas drive streaming display only; `assistant.message` content is
/// authoritative and the final `result` carries session/usage/exit data.
/// Field extraction tries several likely paths so minor shape drift
/// degrades gracefully instead of dropping content.
#[derive(Default)]
pub struct CopilotStreamParser {
    pub session_id: Option<String>,
    pub error: Option<String>,
    /// Concatenated authoritative message contents.
    text: String,
    /// Streamed ephemeral text, display-only fallback.
    delta_buf: String,
    model_reported: bool,
}

impl Decoder for CopilotStreamParser {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        CopilotStreamParser::parse_line(self, line)
    }

    fn finish(&mut self) -> DecodeOutcome {
        let text = std::mem::take(&mut self.text);
        DecodeOutcome {
            error: self.error.take(),
            final_text: if text.is_empty() { None } else { Some(text) },
            fallback_text: std::mem::take(&mut self.delta_buf),
            session_id: self.session_id.clone(),
        }
    }
}

/// First string found at any of the given paths ("a/b" walks objects).
fn extract<'a>(value: &'a Value, paths: &[&str]) -> Option<&'a str> {
    paths.iter().find_map(|path| {
        let mut cursor = value;
        for key in path.split('/') {
            cursor = cursor.get(key)?;
        }
        cursor.as_str()
    })
}

fn extract_u64(value: &Value, paths: &[&str]) -> Option<u64> {
    paths.iter().find_map(|path| {
        let mut cursor = value;
        for key in path.split('/') {
            cursor = cursor.get(key)?;
        }
        cursor.as_u64()
    })
}

impl CopilotStreamParser {
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
            Some("assistant.message_delta") => {
                if let Some(text) = extract(&value, &["content", "delta", "text", "data/content"]) {
                    if !text.is_empty() {
                        self.delta_buf.push_str(text);
                        out.push(AgentEvent::TextDelta {
                            text: text.to_owned(),
                        });
                    }
                }
            }
            Some("assistant.message") => {
                if let Some(content) = extract(&value, &["content", "text", "data/content"]) {
                    if !content.is_empty() {
                        if !self.text.is_empty() {
                            self.text.push_str("\n\n");
                        }
                        self.text.push_str(content);
                    }
                }
                if !self.model_reported {
                    if let Some(model) = extract(&value, &["model", "data/model"]) {
                        self.model_reported = true;
                        out.push(AgentEvent::ModelObserved {
                            model: model.to_owned(),
                        });
                    }
                }
            }
            Some("result") => {
                if let Some(id) = extract(&value, &["sessionId", "session_id", "data/sessionId"]) {
                    self.session_id.get_or_insert_with(|| id.to_owned());
                    out.push(AgentEvent::SessionStarted {
                        session_id: id.to_owned(),
                    });
                }
                out.push(AgentEvent::Usage {
                    input_tokens: extract_u64(&value, &["usage/input_tokens", "usage/inputTokens"]),
                    output_tokens: extract_u64(
                        &value,
                        &["usage/output_tokens", "usage/outputTokens"],
                    ),
                });
                if let Some(code) = extract_u64(&value, &["exitCode", "exit_code"]) {
                    if code != 0 && self.error.is_none() {
                        self.error = Some(format!("copilot reported exit code {code}"));
                    }
                }
            }
            Some("error") => {
                let message = extract(&value, &["message", "error", "data/message"])
                    .unwrap_or("copilot reported an error")
                    .to_owned();
                self.error = Some(message);
            }
            // Bookkeeping, echoes, hidden reasoning, and unknown events:
            // tolerated, never surfaced.
            _ => {}
        }
        out
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

    #[test]
    fn build_args_teammate_golden() {
        assert_eq!(
            build_args(&request(Some("auto"), "ROLE"), None),
            vec![
                "-p",
                "ROLE\n\nevaluate the cache",
                "--output-format",
                "json",
                "--no-color",
                "--no-custom-instructions",
                "--disable-builtin-mcps",
                "--allow-all-tools",
                "--deny-tool",
                "write",
                "--deny-tool",
                "shell",
                "--model",
                "auto",
            ]
        );
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
    fn build_args_drops_deny_tools_for_a_sandboxed_lead() {
        // The write/shell denials are what keep a teammate read-only; a
        // sandboxed lead drops them (it must write `.mix2/` and run
        // `mix2-consult`) and relies on the OS sandbox. `--allow-all-tools`
        // stays — non-interactive mode needs it.
        let args = build_args(&sandboxed(request(Some("auto"), "ROLE")), None);
        assert!(!args.contains(&"--deny-tool".to_owned()));
        assert!(!args.contains(&"write".to_owned()));
        assert!(!args.contains(&"shell".to_owned()));
        assert!(args.contains(&"--allow-all-tools".to_owned()));
    }

    #[test]
    fn build_args_resume_golden() {
        let args = build_args(&request(None, ""), Some("sess-9"));
        assert_eq!(args[1], "evaluate the cache");
        assert_eq!(args.last().unwrap(), "--resume=sess-9");
    }

    #[test]
    fn ephemeral_deltas_stream_but_message_content_is_authoritative() {
        let mut p = CopilotStreamParser::default();
        let ev =
            p.parse_line(r#"{"type":"assistant.message_delta","ephemeral":true,"content":"Hel"}"#);
        assert_eq!(ev, vec![AgentEvent::TextDelta { text: "Hel".into() }]);
        p.parse_line(
            r#"{"type":"assistant.message","content":"Hello there","model":"auto-gpt-5","toolRequests":[],"outputTokens":4}"#,
        );
        p.parse_line(r#"{"type":"result","sessionId":"cs-1","exitCode":0,"usage":{"input_tokens":9,"output_tokens":4}}"#);
        let outcome = Decoder::finish(&mut p);
        assert_eq!(outcome.final_text.as_deref(), Some("Hello there"));
        assert_eq!(outcome.session_id.as_deref(), Some("cs-1"));
        assert!(outcome.error.is_none());
    }

    #[test]
    fn model_is_observed_once_from_the_message() {
        let mut p = CopilotStreamParser::default();
        let ev = p.parse_line(
            r#"{"type":"assistant.message","content":"x","model":"claude-sonnet-4.5"}"#,
        );
        assert!(ev.contains(&AgentEvent::ModelObserved {
            model: "claude-sonnet-4.5".into()
        }));
        assert!(p
            .parse_line(r#"{"type":"assistant.message","content":"y","model":"claude-sonnet-4.5"}"#)
            .is_empty());
    }

    #[test]
    fn nonzero_exit_code_in_result_is_a_failure() {
        let mut p = CopilotStreamParser::default();
        p.parse_line(r#"{"type":"result","sessionId":"cs-2","exitCode":1,"usage":{}}"#);
        assert!(Decoder::finish(&mut p)
            .error
            .as_deref()
            .unwrap()
            .contains("exit code 1"));
    }

    #[test]
    fn session_noise_and_reasoning_are_never_surfaced() {
        let mut p = CopilotStreamParser::default();
        assert!(p
            .parse_line(r#"{"type":"session.mcp_server_status_changed","status":"ok"}"#)
            .is_empty());
        assert!(p
            .parse_line(r#"{"type":"session.skills_loaded","count":9}"#)
            .is_empty());
        assert!(p
            .parse_line(r#"{"type":"assistant.reasoning","content":"secret"}"#)
            .is_empty());
        assert!(p
            .parse_line(r#"{"type":"assistant.turn_start"}"#)
            .is_empty());
    }

    #[test]
    fn deltas_fall_back_when_no_authoritative_message_arrives() {
        let mut p = CopilotStreamParser::default();
        p.parse_line(r#"{"type":"assistant.message_delta","ephemeral":true,"content":"partial"}"#);
        let outcome = Decoder::finish(&mut p);
        assert_eq!(outcome.final_text, None);
        assert_eq!(outcome.fallback_text, "partial");
    }

    #[test]
    fn malformed_lines_warn_without_failing() {
        let mut p = CopilotStreamParser::default();
        assert!(matches!(
            p.parse_line("stderr noise leaked to stdout").as_slice(),
            [AgentEvent::ParserWarning { .. }]
        ));
        assert!(Decoder::finish(&mut p).error.is_none());
    }

    #[test]
    fn copilot_is_teammate_only_by_capability() {
        assert!(!DESCRIPTOR.capabilities.lead_eligible());
        assert!(DESCRIPTOR.capabilities.teammate_eligible());
    }
}
