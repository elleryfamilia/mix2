pub mod commands;
pub mod events;

pub use commands::Command;
pub use events::{AgentInfo, Event, Speaker};

/// Version of the JSONL protocol between the Ink UI and this core.
pub const PROTOCOL_VERSION: u32 = 1;
