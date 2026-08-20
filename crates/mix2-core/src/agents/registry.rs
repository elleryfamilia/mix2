//! The adapter registry: one lookup from [`HarnessKind`] to its descriptor.
//!
//! This is the single place a new harness plugs in: add its module with a
//! `DESCRIPTOR` static (plus decoder and builders), extend `HarnessKind`,
//! and match it here. The runtime, config, and UI stay untouched.

use super::descriptor::Descriptor;
use super::HarnessKind;

/// Every registered harness, in display order.
pub const ALL: [HarnessKind; 2] = [HarnessKind::Claude, HarnessKind::Codex];

pub fn descriptor(harness: HarnessKind) -> &'static Descriptor {
    match harness {
        HarnessKind::Claude => &super::claude::DESCRIPTOR,
        HarnessKind::Codex => &super::codex::DESCRIPTOR,
    }
}

/// Resolve a user-facing harness name ("codex", "Codex"). The registry —
/// not the UI — owns harness-name validation, so error text always reflects
/// what is actually registered.
pub fn harness_named(name: &str) -> Option<HarnessKind> {
    let norm = name.to_ascii_lowercase();
    ALL.into_iter()
        .find(|h| norm == h.to_string() || norm == h.display_name().to_lowercase())
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
        assert_eq!(harness_named("gemini"), None);
        let msg = unknown_harness_message("gemini");
        assert!(msg.contains("gemini"));
        assert!(msg.contains("claude, codex"));
    }
}
