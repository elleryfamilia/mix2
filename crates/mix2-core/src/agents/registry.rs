//! The adapter registry: one lookup from [`HarnessKind`] to its descriptor.
//!
//! This is the single place a new harness plugs in: add its module with a
//! `DESCRIPTOR` static (plus decoder and builders), extend `HarnessKind`,
//! and match it here. The runtime, config, and UI stay untouched.

use super::descriptor::Descriptor;
use super::HarnessKind;

/// Every registered harness, in display order.
pub const ALL: [HarnessKind; 5] = [
    HarnessKind::Claude,
    HarnessKind::Codex,
    HarnessKind::Copilot,
    HarnessKind::Cursor,
    HarnessKind::Opencode,
];

pub fn descriptor(harness: HarnessKind) -> &'static Descriptor {
    match harness {
        HarnessKind::Claude => &super::claude::DESCRIPTOR,
        HarnessKind::Codex => &super::codex::DESCRIPTOR,
        HarnessKind::Copilot => &super::copilot::DESCRIPTOR,
        HarnessKind::Cursor => &super::cursor::DESCRIPTOR,
        HarnessKind::Opencode => &super::opencode::DESCRIPTOR,
    }
}

/// Whether a (lowercased) user-facing name refers to this harness: its
/// canonical name, its display name, or a descriptor alias (the binary
/// name, e.g. "cursor-agent"). The single matching authority — FromStr,
/// slot resolution, and config validation all route through it.
pub fn name_matches(harness: HarnessKind, norm: &str) -> bool {
    norm == harness.to_string()
        || norm == harness.display_name().to_lowercase()
        || descriptor(harness).aliases.contains(&norm)
}

/// Resolve a user-facing harness name ("codex", "Codex", "cursor-agent").
/// The registry — not the UI — owns harness-name validation, so error text
/// always reflects what is actually registered.
pub fn harness_named(name: &str) -> Option<HarnessKind> {
    let norm = name.to_ascii_lowercase();
    ALL.into_iter().find(|h| name_matches(*h, &norm))
}

/// Error text for a name no registered harness answers to.
pub fn unknown_harness_message(name: &str) -> String {
    let known: Vec<String> = ALL.iter().map(|h| h.to_string()).collect();
    format!("unknown harness '{name}' (known: {})", known.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_match_their_harness() {
        for harness in [HarnessKind::Claude, HarnessKind::Codex] {
            let d = descriptor(harness);
            assert_eq!(d.harness, harness);
            assert_eq!(d.label, harness.to_string());
        }
    }

    #[test]
    fn env_overrides_and_defaults_are_distinct() {
        let claude = descriptor(HarnessKind::Claude);
        let codex = descriptor(HarnessKind::Codex);
        assert_eq!(claude.command_env_override, "MIX2_CLAUDE_CMD");
        assert_eq!(codex.command_env_override, "MIX2_CODEX_CMD");
        assert_eq!(claude.default_command, "claude");
        assert_eq!(codex.default_command, "codex");
    }

    #[test]
    fn harness_names_resolve_case_insensitively() {
        assert_eq!(harness_named("codex"), Some(HarnessKind::Codex));
        assert_eq!(harness_named("Claude"), Some(HarnessKind::Claude));
        assert_eq!(harness_named("cursor"), Some(HarnessKind::Cursor));
        assert_eq!(harness_named("gemini"), None);
        let msg = unknown_harness_message("gemini");
        assert!(msg.contains("gemini"));
        assert!(msg.contains("claude, codex, copilot, cursor, opencode"));
    }

    #[test]
    fn aliases_resolve_everywhere_names_do() {
        // The binary name is a first-class alias, consistently: FromStr,
        // registry lookup, and slot resolution all accept it.
        assert_eq!(harness_named("cursor-agent"), Some(HarnessKind::Cursor));
        assert_eq!(
            "cursor-agent".parse::<HarnessKind>(),
            Ok(HarnessKind::Cursor)
        );
        use crate::agents::{SlotId, Team};
        let team = Team {
            one: HarnessKind::Claude,
            two: HarnessKind::Cursor,
            lead: SlotId::One,
        };
        assert_eq!(team.slot_named("cursor-agent"), Some(SlotId::Two));
    }
}
