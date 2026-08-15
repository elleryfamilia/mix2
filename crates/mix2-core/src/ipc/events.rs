use crate::agents::{AgentKind, AgentRole};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    Claude,
    Codex,
    Team,
}

impl From<AgentKind> for Speaker {
    fn from(kind: AgentKind) -> Self {
        match kind {
            AgentKind::Claude => Speaker::Claude,
            AgentKind::Codex => Speaker::Codex,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentInfo {
    pub kind: AgentKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// None = the sign-in probe couldn't tell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authenticated: Option<bool>,
    /// Configured/selected model; None = the provider's own default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Models the CLI accepts, for the /model picker (may be empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

/// Events emitted by the core to the Ink UI, one JSON object per line on
/// stdout. Provider-specific JSON never crosses this boundary; everything is
/// normalized here in Rust.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Event {
    #[serde(rename = "ready")]
    Ready {
        protocol: u32,
        session_id: String,
        lead: AgentInfo,
        teammate: AgentInfo,
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
        agent: AgentKind,
        role: AgentRole,
    },
    #[serde(rename = "agent.text_delta")]
    AgentTextDelta {
        turn_id: String,
        agent: AgentKind,
        role: AgentRole,
        text: String,
    },
    #[serde(rename = "agent.tool.started")]
    AgentToolStarted {
        turn_id: String,
        agent: AgentKind,
        role: AgentRole,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    #[serde(rename = "agent.tool.finished")]
    AgentToolFinished {
        turn_id: String,
        agent: AgentKind,
        role: AgentRole,
        name: String,
    },

    #[serde(rename = "consult.started")]
    ConsultStarted {
        turn_id: String,
        agent: AgentKind,
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
        agent: AgentKind,
        index: u32,
        duration_ms: u64,
        text: String,
    },
    #[serde(rename = "consult.failed")]
    ConsultFailed {
        turn_id: String,
        agent: AgentKind,
        index: u32,
        message: String,
    },

    /// An agent's model changed or was observed from its stream.
    /// source: "selected" (user /model) or "observed" (provider reported).
    #[serde(rename = "agent.model")]
    AgentModel {
        agent: AgentKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        source: String,
    },

    #[serde(rename = "lead.synthesizing")]
    LeadSynthesizing { turn_id: String, agent: AgentKind },

    #[serde(rename = "message.final")]
    MessageFinal {
        turn_id: String,
        speaker: Speaker,
        lead: AgentKind,
        text: String,
        consultations: u32,
        duration_ms: u64,
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
            agent: AgentKind::Codex,
            index: 1,
            max: 2,
            prompt: "evaluate X".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(
            json,
            r#"{"type":"consult.started","turn_id":"t1","agent":"codex","index":1,"max":2,"prompt":"evaluate X"}"#
        );
    }

    #[test]
    fn round_trips() {
        let ev = Event::MessageFinal {
            turn_id: "t1".into(),
            speaker: Speaker::Team,
            lead: AgentKind::Claude,
            text: "answer".into(),
            consultations: 1,
            duration_ms: 1234,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn speaker_from_kind() {
        assert_eq!(Speaker::from(AgentKind::Claude), Speaker::Claude);
        assert_eq!(Speaker::from(AgentKind::Codex), Speaker::Codex);
    }
}
