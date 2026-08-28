use serde::{Deserialize, Serialize};

/// Commands sent from the Ink UI to the core, one JSON object per line on
/// the core's stdin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Must be the first command. The core runs discovery and replies with
    /// `harnesses.discovered`, then `ready` (after auto-confirmation or a
    /// `select_team`) or `fatal`.
    Initialize {
        protocol: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        lead: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default)]
        debug: bool,
        /// A human is present: without an explicit `[slot.*]` config, the
        /// core waits for `select_team` instead of auto-confirming.
        #[serde(default)]
        interactive: bool,
        /// Force the selection handshake even with an explicit config.
        #[serde(default)]
        pick_team: bool,
    },
    /// Settle the team while the core is awaiting selection: a harness per
    /// slot plus the lead slot. Slot ids are canonical (`one`/`two`);
    /// harness values are registry names.
    SelectTeam {
        one: String,
        two: String,
        lead_slot: String,
        /// The picker's consultation budget ("turns"); absent from older
        /// UIs, which keep the configured value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_turns: Option<u32>,
    },
    /// Start a user turn. `id` is the UI's correlation id, echoed as
    /// `turn_id` on every event for this turn.
    Submit {
        id: String,
        text: String,
    },
    Cancel {
        turn_id: String,
    },
    /// Override (or clear, with model=None) the model a slot uses for
    /// subsequent invocations this session. `slot` is `one`/`two`, or a
    /// harness name while it names exactly one slot.
    SetModel {
        slot: String,
        #[serde(default)]
        model: Option<String>,
    },
    /// `/turns <n>`: set the per-turn consultation budget for the rest of
    /// this session (from the next turn) and persist it to the config.
    SetTurns {
        max: u32,
    },
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_initialize() {
        let cmd: Command = serde_json::from_str(
            r#"{"type":"initialize","protocol":4,"lead":"claude","cwd":"/r","interactive":true}"#,
        )
        .unwrap();
        assert_eq!(
            cmd,
            Command::Initialize {
                protocol: 4,
                lead: Some("claude".into()),
                cwd: Some("/r".into()),
                debug: false,
                interactive: true,
                pick_team: false,
            }
        );
    }

    #[test]
    fn parses_select_team() {
        let cmd: Command = serde_json::from_str(
            r#"{"type":"select_team","one":"codex","two":"codex","lead_slot":"two"}"#,
        )
        .unwrap();
        assert_eq!(
            cmd,
            Command::SelectTeam {
                one: "codex".into(),
                two: "codex".into(),
                lead_slot: "two".into(),
                max_turns: None,
            }
        );
    }

    #[test]
    fn parses_select_team_with_turns_and_set_turns() {
        let cmd: Command = serde_json::from_str(
            r#"{"type":"select_team","one":"claude","two":"codex","lead_slot":"one","max_turns":3}"#,
        )
        .unwrap();
        assert!(matches!(
            cmd,
            Command::SelectTeam {
                max_turns: Some(3),
                ..
            }
        ));
        let cmd: Command = serde_json::from_str(r#"{"type":"set_turns","max":4}"#).unwrap();
        assert_eq!(cmd, Command::SetTurns { max: 4 });
    }

    #[test]
    fn parses_submit_and_cancel() {
        let cmd: Command =
            serde_json::from_str(r#"{"type":"submit","id":"t1","text":"hi"}"#).unwrap();
        assert_eq!(
            cmd,
            Command::Submit {
                id: "t1".into(),
                text: "hi".into()
            }
        );
        let cmd: Command = serde_json::from_str(r#"{"type":"cancel","turn_id":"t1"}"#).unwrap();
        assert_eq!(
            cmd,
            Command::Cancel {
                turn_id: "t1".into()
            }
        );
    }

    #[test]
    fn parses_set_model_by_slot() {
        let cmd: Command =
            serde_json::from_str(r#"{"type":"set_model","slot":"one","model":"sonnet"}"#).unwrap();
        assert_eq!(
            cmd,
            Command::SetModel {
                slot: "one".into(),
                model: Some("sonnet".into())
            }
        );
    }

    #[test]
    fn rejects_unknown_command() {
        assert!(serde_json::from_str::<Command>(r#"{"type":"reboot"}"#).is_err());
    }
}
