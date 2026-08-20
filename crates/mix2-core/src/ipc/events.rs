use crate::agents::agent::AuthState;
pub use crate::agents::discovery::DiscoveredHarness;
use crate::agents::{AgentRole, HarnessKind, SlotId};
pub use crate::collaboration::disagreement::{DisagreementRecord, Outcome, Stance};
use serde::{Deserialize, Serialize};

/// Who a settled message speaks for: one slot alone, or the whole team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    One,
    Two,
    Team,
}

impl From<SlotId> for Speaker {
    fn from(slot: SlotId) -> Self {
        match slot {
            SlotId::One => Speaker::One,
            SlotId::Two => Speaker::Two,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentInfo {
    /// The durable participant identity this info describes.
    pub slot: SlotId,
    /// Which provider CLI backs the slot. Display only — behavior never
    /// branches on the *other* slot's harness.
    pub harness: HarnessKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Five-state sign-in probe result; only `unauthenticated` ever blocks.
    pub auth: AuthState,
    /// Configured/selected model; None = the provider's own default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Models the CLI accepts, for the /model picker (may be empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

/// The core's configured/default team shape, sent with discovery so a
/// picker can preselect it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamProposal {
    pub one: HarnessKind,
    pub two: HarnessKind,
    pub lead_slot: SlotId,
}

/// Events emitted by the core to the Ink UI, one JSON object per line on
/// stdout. Provider-specific JSON never crosses this boundary; everything is
/// normalized here in Rust and keyed by [`SlotId`], never by harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Event {
    /// Startup discovery report: every probed `(harness, command)` pair,
    /// the configured proposal, and whether the core is auto-confirming it
    /// (`auto: true`) or waiting for a `select_team` command.
    #[serde(rename = "harnesses.discovered")]
    HarnessesDiscovered {
        harnesses: Vec<DiscoveredHarness>,
        proposal: TeamProposal,
        auto: bool,
    },

    #[serde(rename = "ready")]
    Ready {
        protocol: u32,
        session_id: String,
        one: Box<AgentInfo>,
        two: Box<AgentInfo>,
        lead_slot: SlotId,
        cwd: String,
        /// Whether the cwd looks like a software project; false switches the
        /// team into general-brainstorming framing.
        project: bool,
    },
    /// Unrecoverable startup or runtime failure. The UI shows it and exits.
    #[serde(rename = "fatal")]
    Fatal { message: String },

    #[serde(rename = "message.user")]
    MessageUser { turn_id: String, text: String },

    #[serde(rename = "turn.started")]
    TurnStarted { turn_id: String },

    #[serde(rename = "agent.started")]
    AgentStarted {
        turn_id: String,
        slot: SlotId,
        role: AgentRole,
    },
    #[serde(rename = "agent.text_delta")]
    AgentTextDelta {
        turn_id: String,
        slot: SlotId,
        role: AgentRole,
        text: String,
    },
    #[serde(rename = "agent.tool.started")]
    AgentToolStarted {
        turn_id: String,
        slot: SlotId,
        role: AgentRole,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    #[serde(rename = "agent.tool.finished")]
    AgentToolFinished {
        turn_id: String,
        slot: SlotId,
        role: AgentRole,
        name: String,
    },

    #[serde(rename = "consult.started")]
    ConsultStarted {
        turn_id: String,
        slot: SlotId,
        index: u32,
        max: u32,
        /// The lead's written consultation prompt (team panel only).
        prompt: String,
    },
    /// `text` is the teammate's final consultation response — shown only in
    /// the optional team panel, never spliced into the conversation.
    #[serde(rename = "consult.completed")]
    ConsultCompleted {
        turn_id: String,
        slot: SlotId,
        index: u32,
        duration_ms: u64,
        text: String,
    },
    #[serde(rename = "consult.failed")]
    ConsultFailed {
        turn_id: String,
        slot: SlotId,
        index: u32,
        message: String,
    },

    /// A disagreement was committed for this turn — live team-panel ledger
    /// only. The settled UI reads `message.final`'s `disagreement` field
    /// instead, so this event never appears there.
    #[serde(rename = "disagreement.recorded")]
    DisagreementRecorded {
        turn_id: String,
        stances: Vec<Stance>,
        resolution: String,
        revision: u32,
    },

    /// A slot's model changed or was observed from its stream.
    /// source: "selected" (user /model) or "observed" (provider reported).
    #[serde(rename = "agent.model")]
    AgentModel {
        slot: SlotId,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        source: String,
    },

    #[serde(rename = "lead.synthesizing")]
    LeadSynthesizing { turn_id: String, slot: SlotId },

    #[serde(rename = "message.final")]
    MessageFinal {
        turn_id: String,
        speaker: Speaker,
        lead_slot: SlotId,
        text: String,
        consultations: u32,
        duration_ms: u64,
        /// The turn's settled disagreement record, if the lead recorded one.
        #[serde(skip_serializing_if = "Option::is_none")]
        disagreement: Option<DisagreementRecord>,
    },

    #[serde(rename = "turn.completed")]
    TurnCompleted {
        turn_id: String,
        duration_ms: u64,
        consultations: u32,
    },
    #[serde(rename = "turn.cancelled")]
    TurnCancelled { turn_id: String },
    #[serde(rename = "turn.failed")]
    TurnFailed { turn_id: String, message: String },

    /// Non-fatal diagnostics (parser warnings, ignored commands).
    #[serde(rename = "warning")]
    Warning { message: String },
    /// The UI sent something invalid; the session continues.
    #[serde(rename = "error")]
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_serialization_shape() {
        let ev = Event::ConsultStarted {
            turn_id: "t1".into(),
            slot: SlotId::Two,
            index: 1,
            max: 2,
            prompt: "evaluate X".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(
            json,
            r#"{"type":"consult.started","turn_id":"t1","slot":"two","index":1,"max":2,"prompt":"evaluate X"}"#
        );
    }

    #[test]
    fn round_trips() {
        let ev = Event::MessageFinal {
            turn_id: "t1".into(),
            speaker: Speaker::Team,
            lead_slot: SlotId::One,
            text: "answer".into(),
            consultations: 1,
            duration_ms: 1234,
            disagreement: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn speaker_from_slot() {
        assert_eq!(Speaker::from(SlotId::One), Speaker::One);
        assert_eq!(Speaker::from(SlotId::Two), Speaker::Two);
        assert_eq!(serde_json::to_string(&Speaker::One).unwrap(), r#""one""#);
        assert_eq!(serde_json::to_string(&Speaker::Team).unwrap(), r#""team""#);
    }

    #[test]
    fn agent_info_carries_slot_harness_and_auth() {
        let info = AgentInfo {
            slot: SlotId::Two,
            harness: HarnessKind::Codex,
            name: "Codex".into(),
            version: Some("1.0".into()),
            available: true,
            reason: None,
            auth: AuthState::Authenticated,
            model: None,
            models: vec![],
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains(r#""slot":"two""#), "{json}");
        assert!(json.contains(r#""harness":"codex""#), "{json}");
        assert!(json.contains(r#""auth":"authenticated""#), "{json}");
    }

    #[test]
    fn auth_state_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&AuthState::ProbeFailed).unwrap(),
            r#""probe_failed""#
        );
        assert_eq!(
            serde_json::to_string(&AuthState::Unsupported).unwrap(),
            r#""unsupported""#
        );
    }

    #[test]
    fn ready_shape_is_slot_keyed() {
        let info = |slot: SlotId, harness: HarnessKind| {
            Box::new(AgentInfo {
                slot,
                harness,
                name: harness.display_name().to_owned(),
                version: None,
                available: true,
                reason: None,
                auth: AuthState::ProbeFailed,
                model: None,
                models: vec![],
            })
        };
        // A same-harness team serializes without loss: identity is the slot.
        let ev = Event::Ready {
            protocol: 2,
            session_id: "s1".into(),
            one: info(SlotId::One, HarnessKind::Codex),
            two: info(SlotId::Two, HarnessKind::Codex),
            lead_slot: SlotId::Two,
            cwd: "/repo".into(),
            project: true,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""lead_slot":"two""#), "{json}");
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn harnesses_discovered_round_trips() {
        use crate::agents::descriptor::{Capabilities, CapabilityLevel};
        let ev = Event::HarnessesDiscovered {
            harnesses: vec![DiscoveredHarness {
                harness: HarnessKind::Codex,
                command: "codex".into(),
                version: Some("0.146.0".into()),
                auth: AuthState::Authenticated,
                available: true,
                reason: None,
                note: None,
                lead_eligible: true,
                teammate_eligible: true,
                capabilities: Capabilities {
                    teammate_read_only: CapabilityLevel::Enforced,
                    lead_permission_scoping: CapabilityLevel::Unverified,
                    instruction_injection: CapabilityLevel::Enforced,
                },
            }],
            proposal: TeamProposal {
                one: HarnessKind::Claude,
                two: HarnessKind::Codex,
                lead_slot: SlotId::One,
            },
            auto: true,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""type":"harnesses.discovered""#), "{json}");
        assert!(
            json.contains(r#""teammate_read_only":"enforced""#),
            "{json}"
        );
        assert!(json.contains(r#""lead_slot":"one""#), "{json}");
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn disagreement_recorded_shape() {
        let ev = Event::DisagreementRecorded {
            turn_id: "t1".into(),
            stances: vec![
                Stance {
                    slot: SlotId::One,
                    position: "cache in-process".into(),
                    outcome: Outcome::Chosen,
                },
                Stance {
                    slot: SlotId::Two,
                    position: "move validation off the hot path".into(),
                    outcome: Outcome::Deferred,
                },
            ],
            resolution: "ship the cache; file the rework".into(),
            revision: 1,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(
            json,
            r#"{"type":"disagreement.recorded","turn_id":"t1","stances":[{"slot":"one","position":"cache in-process","outcome":"chosen"},{"slot":"two","position":"move validation off the hot path","outcome":"deferred"}],"resolution":"ship the cache; file the rework","revision":1}"#
        );
    }

    #[test]
    fn message_final_omits_disagreement_when_none() {
        let ev = Event::MessageFinal {
            turn_id: "t1".into(),
            speaker: Speaker::Team,
            lead_slot: SlotId::One,
            text: "answer".into(),
            consultations: 1,
            duration_ms: 1234,
            disagreement: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            !json.contains("disagreement"),
            "key should be omitted when None: {json}"
        );
    }

    #[test]
    fn message_final_disagreement_round_trips() {
        let ev = Event::MessageFinal {
            turn_id: "t1".into(),
            speaker: Speaker::Team,
            lead_slot: SlotId::One,
            text: "answer".into(),
            consultations: 1,
            duration_ms: 1234,
            disagreement: Some(DisagreementRecord {
                stances: vec![Stance {
                    slot: SlotId::One,
                    position: "cache in-process".into(),
                    outcome: Outcome::Chosen,
                }],
                resolution: "ship the cache".into(),
            }),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""disagreement":{"stances":"#));
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }
}
