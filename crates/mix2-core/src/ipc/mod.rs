pub mod commands;
pub mod events;

pub use commands::Command;
pub use events::{AgentInfo, Event, Speaker, TeamProposal};

/// Version of the JSONL protocol between the Ink UI and this core.
/// v2: participant identity moved from harness names to slots — events carry
/// `slot`/`lead_slot`, `speaker` is `one|two|team`, `set_model` targets a
/// slot, and `ready` reports both slots plus the lead slot.
/// v3: the startup handshake — `harnesses.discovered` precedes `ready`,
/// `select_team` settles the team when the core isn't auto-confirming, and
/// `AgentInfo` reports a five-state `auth`.
pub const PROTOCOL_VERSION: u32 = 3;
