use super::AgentKind;

/// Provider-neutral events emitted while an agent invocation runs.
///
/// These are normalized from each provider's native stream. Only semantics
/// that both providers genuinely share are modeled; anything else stays in
/// the adapter. Hidden reasoning is never surfaced here.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    Started {
        agent: AgentKind,
    },
    /// The provider reported its native session/thread id.
    SessionStarted {
        agent: AgentKind,
        session_id: String,
    },
    /// Incremental assistant text (safe to render).
    TextDelta {
        agent: AgentKind,
        text: String,
    },
    /// A complete assistant message (safe to render).
    Message {
        agent: AgentKind,
        text: String,
    },
    ToolStarted {
        agent: AgentKind,
        name: String,
        detail: Option<String>,
    },
    ToolFinished {
        agent: AgentKind,
        name: String,
    },
    Usage {
        agent: AgentKind,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    Completed {
        agent: AgentKind,
    },
    Failed {
        agent: AgentKind,
        message: String,
    },
    /// A line the parser could not understand. Logged, never fatal.
    ParserWarning {
        agent: AgentKind,
        message: String,
    },
}
