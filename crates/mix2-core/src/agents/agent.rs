use super::{AgentEvent, AgentRole, HarnessKind};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentVersion {
    pub raw: String,
}

/// Result of a cheap, quota-free sign-in probe. Only an explicit
/// `Unauthenticated` blocks startup; every other state surfaces as
/// information (reasons in discovery, picker labels) without gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Authenticated,
    Unauthenticated,
    /// Credentials are present but their live validity is unverified
    /// (an inventory listing, not a session check).
    Configured,
    /// The CLI has no quota-free probe; sign-in state is unknowable here.
    Unsupported,
    /// The probe errored or timed out; don't block on it.
    ProbeFailed,
}

#[derive(Debug, Clone)]
pub struct AgentSession {
    pub harness: HarnessKind,
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub prompt: String,
    pub cwd: PathBuf,
    pub role: AgentRole,
    pub turn_id: Uuid,
    /// Model override; None uses the provider's own default.
    pub model: Option<String>,
    /// Role instructions appended to the provider's own system prompt.
    pub instructions: String,
    /// Extra environment for the spawned CLI (MIX2_* markers).
    pub env: HashMap<String, String>,
    /// Directory prepended to PATH so `mix2-consult` resolves.
    pub path_prepend: Option<PathBuf>,
    /// Runtime dir that must stay writable for consult file IPC (codex lead).
    pub runtime_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct AgentResult {
    /// The agent's final user-facing message.
    pub text: String,
    /// Native provider session id, when one was observed.
    pub session_id: Option<String>,
}

/// A provider adapter. All provider-specific invocation behavior lives behind
/// this trait; the runtime and UI never see raw provider JSON.
#[async_trait]
pub trait Agent: Send + Sync {
    fn harness(&self) -> HarnessKind;

    fn display_name(&self) -> &'static str {
        self.harness().display_name()
    }

    /// Resolve and report the installed CLI version, or fail if missing.
    async fn version(&self) -> Result<AgentVersion>;

    /// Cheap sign-in probe (no model quota). Defaults to Unsupported.
    async fn auth_status(&self) -> AuthState {
        AuthState::Unsupported
    }

    /// Models this provider's CLI accepts, for the /model picker. Curated
    /// per adapter; empty means "unknown — provider default only".
    fn known_models(&self) -> Vec<String> {
        Vec::new()
    }

    /// The model list actually offered at startup: live enumeration when
    /// the CLI supports it (bounded, quota-free), else `known_models`.
    /// Enumeration failure falls back — it never gates availability.
    async fn models(&self) -> Vec<String> {
        self.known_models()
    }

    /// Start a fresh provider session.
    async fn start(
        &self,
        request: AgentRequest,
        events: Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<AgentResult>;

    /// Continue an existing provider session.
    async fn resume(
        &self,
        session: &AgentSession,
        request: AgentRequest,
        events: Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<AgentResult>;
}
