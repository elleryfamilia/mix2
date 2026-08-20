/// Provider-neutral events emitted while an agent invocation runs.
///
/// These are normalized from each provider's native stream. Only semantics
/// that both providers genuinely share are modeled; anything else stays in
/// the adapter. Deliberately identity-free: the decoder cannot know which
/// team slot it speaks for, so the runtime stamps the [`crate::agents::SlotId`]
/// when it forwards these to the UI. Hidden reasoning is never surfaced here.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    Started,
    /// The provider reported its native session/thread id.
    SessionStarted {
        session_id: String,
    },
    /// Incremental assistant text (safe to render).
    TextDelta {
        text: String,
    },
    /// A complete assistant message (safe to render).
    Message {
        text: String,
    },
    ToolStarted {
        name: String,
        detail: Option<String>,
    },
    ToolFinished {
        name: String,
    },
    /// The provider reported which model is actually serving this run.
    ModelObserved {
        model: String,
    },
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    Completed,
    Failed {
        message: String,
    },
    /// A line the parser could not understand. Logged, never fatal.
    ParserWarning {
        message: String,
    },
}
