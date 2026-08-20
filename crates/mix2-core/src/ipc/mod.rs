pub mod commands;
pub mod events;

pub use commands::Command;
pub use events::{AgentInfo, Event, Speaker};

/// Version of the JSONL protocol between the Ink UI and this core.
/// v2: participant identity moved from harness names to slots — events carry
/// `slot`/`lead_slot`, `speaker` is `one|two|team`, `set_model` targets a
/// slot, and `ready` reports both slots plus the lead slot.
pub const PROTOCOL_VERSION: u32 = 2;
