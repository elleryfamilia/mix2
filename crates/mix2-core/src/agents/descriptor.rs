//! The declarative half of a harness adapter.
//!
//! A [`Descriptor`] is everything mix2 knows about one provider CLI without
//! running it: metadata and hints, capability facts, and the pure functions
//! (argument builder, version parser, decoder factory) that the shared
//! runner drives. Adding a harness means writing a descriptor + decoder and
//! registering it in `registry.rs` — the runner and runtime never change.

use super::agent::AgentRequest;
use super::{AgentEvent, HarnessKind};
use serde::{Deserialize, Serialize};

/// How firmly a capability holds for a harness.
///
/// `Enforced` means the runtime can rely on it mechanically (a sandbox or
/// permission flag guarantees it); `Unverified` means it currently rests on
/// instructions or the user's own CLI configuration; `Unsupported` means the
/// harness has no way to provide it. Role eligibility derives from these —
/// they are facts, not booleans, so a future harness can be honest about
/// partial support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityLevel {
    Enforced,
    Unverified,
    Unsupported,
}

/// The capability facts that decide what roles a harness may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// A teammate consultation cannot write to the project.
    pub teammate_read_only: CapabilityLevel,
    /// A lead's writes are scoped to the team scratchpad (`.mix2/`).
    pub lead_permission_scoping: CapabilityLevel,
    /// mix2's role instructions reach the model on top of the CLI's own
    /// system prompt.
    pub instruction_injection: CapabilityLevel,
}

impl Capabilities {
    /// Role eligibility derives from capability facts: a role is open
    /// unless some capability it depends on is outright unsupported.
    /// `Unverified` does not disqualify — it surfaces as information.
    pub fn lead_eligible(&self) -> bool {
        self.instruction_injection != CapabilityLevel::Unsupported
            && self.lead_permission_scoping != CapabilityLevel::Unsupported
    }

    pub fn teammate_eligible(&self) -> bool {
        self.instruction_injection != CapabilityLevel::Unsupported
            && self.teammate_read_only != CapabilityLevel::Unsupported
    }
}

/// Quota-free sign-in probes, declaratively. The runner interprets these;
/// probe styles that new harnesses need become new variants here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthProbe {
    /// Run the CLI with `args`; stdout carries JSON with a boolean
    /// `loggedIn` field (banner lines before the JSON are tolerated).
    JsonLoggedIn { args: &'static [&'static str] },
    /// Run the CLI with `args`; exit 0 means signed in, non-zero signed out.
    ExitStatus { args: &'static [&'static str] },
    /// No quota-free probe exists; auth state is `Unsupported` and runtime
    /// failures must surface cleanly instead (never trial prompts).
    None,
}

/// Terminal state a decoder accumulated over one invocation's stream,
/// consumed once by the runner after the child exits.
#[derive(Debug, Default)]
pub struct DecodeOutcome {
    /// A provider-reported error; set means the turn failed even if the
    /// process exited zero.
    pub error: Option<String>,
    /// The authoritative final message, when the stream carried one.
    pub final_text: Option<String>,
    /// Best-effort text when the stream ended without a final message
    /// (e.g. accumulated deltas). May be empty.
    pub fallback_text: String,
    /// Native provider session/thread id, when one was observed.
    pub session_id: Option<String>,
}

/// A streaming decoder for one provider invocation: raw stdout lines in,
/// identity-free [`AgentEvent`]s out, terminal state via [`Decoder::finish`].
/// Must tolerate unknown event types and malformed lines by construction.
pub trait Decoder: Send {
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent>;
    fn finish(&mut self) -> DecodeOutcome;
}

/// Everything the shared runner needs to drive one harness.
pub struct Descriptor {
    pub harness: HarnessKind,
    /// Lowercase label used in error messages and logs ("claude").
    pub label: &'static str,
    /// Binary invoked when neither config nor env override one.
    pub default_command: &'static str,
    /// Test/dev env var that overrides the command (fake fixtures).
    pub command_env_override: &'static str,
    /// Actionable fix shown when the CLI is missing.
    pub install_hint: &'static str,
    /// Actionable fix shown when the CLI is signed out.
    pub login_hint: &'static str,
    pub capabilities: Capabilities,
    /// Curated models for the /model picker; empty means "provider default
    /// only". Swapped for live discovery when a CLI grows a listing command.
    pub known_models: &'static [&'static str],
    /// Arguments of the version probe (typically `--version`).
    pub version_args: &'static [&'static str],
    /// Extract the version line from the probe's stdout.
    pub parse_version: fn(&str) -> String,
    pub auth_probe: AuthProbe,
    /// Build the full argv for one invocation; `resume` carries the native
    /// session id when continuing a conversation.
    pub build_args: fn(&AgentRequest, Option<&str>) -> Vec<String>,
    pub new_decoder: fn() -> Box<dyn Decoder>,
}
