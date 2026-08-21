//! mix2 core: agent runtime, process management, sessions, collaboration,
//! limits, provider parsing, and cancellation. The Ink TUI is presentation
//! only; everything with side effects lives here.

pub mod agents;
pub mod collaboration;
pub mod config;
pub mod ipc;
pub mod process;
pub mod runtime;
pub mod sandbox;
pub mod session;
