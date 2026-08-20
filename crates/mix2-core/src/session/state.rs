use crate::agents::Team;
use std::path::PathBuf;
use uuid::Uuid;

/// One mix2 session: a TUI process talking to one lead across many turns.
///
/// The lead keeps its native provider conversation across turns via
/// `lead_provider_session_id`; a brand-new mix2 session starts with `None`
/// so it can never accidentally resume an older provider session. The
/// teammate deliberately has no persistent conversational identity in the
/// MVP: consultations are independent fresh sessions.
#[derive(Debug, Clone)]
pub struct Mix2Session {
    pub id: Uuid,
    pub team: Team,
    pub cwd: PathBuf,
    pub lead_provider_session_id: Option<String>,
}

impl Mix2Session {
    pub fn new(team: Team, cwd: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            team,
            cwd,
            lead_provider_session_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}
