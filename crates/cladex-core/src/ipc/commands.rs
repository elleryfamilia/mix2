use serde::{Deserialize, Serialize};

/// Commands sent from the Ink UI to the core, one JSON object per line on
/// the core's stdin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Must be the first command. The core probes providers and replies with
    /// `ready` or `fatal`.
    Initialize {
        protocol: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        lead: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default)]
        debug: bool,
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
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_initialize() {
        let cmd: Command = serde_json::from_str(
            r#"{"type":"initialize","protocol":1,"lead":"claude","cwd":"/r"}"#,
        )
        .unwrap();
        assert_eq!(
            cmd,
            Command::Initialize {
                protocol: 1,
                lead: Some("claude".into()),
                cwd: Some("/r".into()),
                debug: false
            }
        );
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
    fn rejects_unknown_command() {
        assert!(serde_json::from_str::<Command>(r#"{"type":"reboot"}"#).is_err());
    }
}
