//! The adapter registry: one lookup from [`HarnessKind`] to its descriptor.
//!
//! This is the single place a new harness plugs in: add its module with a
//! `DESCRIPTOR` static (plus decoder and builders), extend `HarnessKind`,
//! and match it here. The runtime, config, and UI stay untouched.

use super::descriptor::Descriptor;
use super::HarnessKind;

pub fn descriptor(harness: HarnessKind) -> &'static Descriptor {
    match harness {
        HarnessKind::Claude => &super::claude::DESCRIPTOR,
        HarnessKind::Codex => &super::codex::DESCRIPTOR,
    }
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
}
