pub mod agent;
pub mod claude;
pub mod codex;
pub mod events;

pub use agent::{Agent, AgentRequest, AgentResult, AgentSession, AgentVersion};
pub use events::AgentEvent;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The durable participant identity. A mix2 session always has exactly two
/// slots; sessions, IPC events, model selection, disagreement stances, and
/// TUI colors/glyphs all key on the slot. Which CLI backs a slot is the
/// separately-chosen [`HarnessKind`] — never infer one from the other, and
/// never assume the two slots run different harnesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlotId {
    One,
    Two,
}

impl SlotId {
    pub const ALL: [SlotId; 2] = [SlotId::One, SlotId::Two];

    /// The other member of the two-slot team.
    pub fn other(self) -> SlotId {
        match self {
            SlotId::One => SlotId::Two,
            SlotId::Two => SlotId::One,
        }
    }
}

impl fmt::Display for SlotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlotId::One => write!(f, "one"),
            SlotId::Two => write!(f, "two"),
        }
    }
}

impl FromStr for SlotId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "one" => Ok(SlotId::One),
            "two" => Ok(SlotId::Two),
            other => Err(format!("unknown slot '{other}' (expected 'one' or 'two')")),
        }
    }
}

/// Which provider CLI backs a slot. Selects behavior only (invocation,
/// decoding, probes); participant identity lives in [`SlotId`]. New
/// providers extend this enum plus the adapter registry in `for_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessKind {
    Claude,
    Codex,
}

impl HarnessKind {
    pub fn display_name(self) -> &'static str {
        match self {
            HarnessKind::Claude => "Claude",
            HarnessKind::Codex => "Codex",
        }
    }
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HarnessKind::Claude => write!(f, "claude"),
            HarnessKind::Codex => write!(f, "codex"),
        }
    }
}

impl FromStr for HarnessKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "claude" => Ok(HarnessKind::Claude),
            "codex" => Ok(HarnessKind::Codex),
            other => Err(format!(
                "unknown agent '{other}' (expected 'claude' or 'codex')"
            )),
        }
    }
}

/// The resolved team shape: which harness backs each slot, and which slot
/// leads. Copy-cheap; passed wherever participant identity matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Team {
    pub one: HarnessKind,
    pub two: HarnessKind,
    pub lead: SlotId,
}

impl Team {
    pub fn harness(&self, slot: SlotId) -> HarnessKind {
        match slot {
            SlotId::One => self.one,
            SlotId::Two => self.two,
        }
    }

    pub fn teammate(&self) -> SlotId {
        self.lead.other()
    }

    pub fn lead_harness(&self) -> HarnessKind {
        self.harness(self.lead)
    }

    pub fn teammate_harness(&self) -> HarnessKind {
        self.harness(self.teammate())
    }

    /// Resolve a user-facing participant name to a slot. `one`/`two` always
    /// work; a harness name or display name ("codex", "Claude") works only
    /// while exactly one slot runs that harness — on a same-harness team the
    /// name is ambiguous and only the slot ids resolve.
    pub fn slot_named(&self, name: &str) -> Option<SlotId> {
        if let Ok(slot) = name.parse::<SlotId>() {
            return Some(slot);
        }
        let norm = name.to_ascii_lowercase();
        let matches: Vec<SlotId> = SlotId::ALL
            .into_iter()
            .filter(|&slot| {
                let harness = self.harness(slot);
                norm == harness.to_string() || norm == harness.display_name().to_lowercase()
            })
            .collect();
        match matches.as_slice() {
            [slot] => Some(*slot),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    Lead,
    Teammate,
}

impl fmt::Display for AgentRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentRole::Lead => write!(f, "lead"),
            AgentRole::Teammate => write!(f, "teammate"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_id_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&SlotId::One).unwrap(), r#""one""#);
        assert_eq!(serde_json::to_string(&SlotId::Two).unwrap(), r#""two""#);
        assert_eq!(
            serde_json::from_str::<SlotId>(r#""two""#).unwrap(),
            SlotId::Two
        );
    }

    #[test]
    fn harness_kind_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&HarnessKind::Claude).unwrap(),
            r#""claude""#
        );
        assert_eq!(
            serde_json::from_str::<HarnessKind>(r#""codex""#).unwrap(),
            HarnessKind::Codex
        );
    }

    #[test]
    fn slot_id_parses_and_displays() {
        assert_eq!("one".parse::<SlotId>().unwrap(), SlotId::One);
        assert_eq!("TWO".parse::<SlotId>().unwrap(), SlotId::Two);
        assert!("three".parse::<SlotId>().is_err());
        assert_eq!(SlotId::One.to_string(), "one");
        assert_eq!(SlotId::One.other(), SlotId::Two);
    }

    fn mixed_team() -> Team {
        Team {
            one: HarnessKind::Claude,
            two: HarnessKind::Codex,
            lead: SlotId::One,
        }
    }

    fn same_harness_team() -> Team {
        Team {
            one: HarnessKind::Codex,
            two: HarnessKind::Codex,
            lead: SlotId::Two,
        }
    }

    #[test]
    fn team_resolves_roles_by_slot() {
        let team = Team {
            lead: SlotId::Two,
            ..mixed_team()
        };
        assert_eq!(team.lead_harness(), HarnessKind::Codex);
        assert_eq!(team.teammate(), SlotId::One);
        assert_eq!(team.teammate_harness(), HarnessKind::Claude);
    }

    #[test]
    fn slot_named_accepts_ids_and_unique_harness_names() {
        let team = mixed_team();
        assert_eq!(team.slot_named("one"), Some(SlotId::One));
        assert_eq!(team.slot_named("Two"), Some(SlotId::Two));
        assert_eq!(team.slot_named("claude"), Some(SlotId::One));
        assert_eq!(team.slot_named("Codex"), Some(SlotId::Two));
        assert_eq!(team.slot_named("gemini"), None);
    }

    #[test]
    fn slot_named_rejects_ambiguous_names_on_same_harness_teams() {
        let team = same_harness_team();
        assert_eq!(team.slot_named("codex"), None, "ambiguous by name");
        assert_eq!(team.slot_named("one"), Some(SlotId::One));
        assert_eq!(team.slot_named("two"), Some(SlotId::Two));
    }
}
