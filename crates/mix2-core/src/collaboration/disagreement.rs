//! Grammar and record types for the disagreement layer.
//!
//! The lead agent records a genuine split with its teammate through the
//! `mix2-consult disagree` CLI surface. The helper binary stays dumb (std +
//! serde_json only) and forwards the payload verbatim; ALL parsing and
//! validation happens here, server-side, so the grammar lives in exactly one
//! place. This module stays dependency-free (serde + std only).

use crate::agents::{SlotId, Team};
use serde::{Deserialize, Serialize};

/// What became of one agent's position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Chosen,
    Deferred,
    Dropped,
}

/// One team slot's position and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stance {
    pub slot: SlotId,
    pub position: String,
    pub outcome: Outcome,
}

/// A recorded disagreement: each slot's stance plus the team's resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisagreementRecord {
    pub stances: Vec<Stance>,
    pub resolution: String,
}

/// A filled-in, valid `mix2-consult disagree` payload. Interpolated into the
/// lead prompt so refusals (and the prompt itself) always show a real,
/// working example — `example_constant_parses` below proves it parses.
pub const DISAGREE_EXAMPLE: &str = r#"mix2-consult disagree <<'SPLIT'
claude: cache the compiled schema in-process | chosen
codex: move validation off the hot path | deferred
team: ship the cache now; file the validation rework as a follow-up
SPLIT"#;

/// The worked example whose agent lines actually parse for this team:
/// harness names while they are unambiguous, slot ids on a same-harness
/// team. `examples_parse_for_their_team` proves both forms parse.
pub fn example_for(team: &Team) -> String {
    let (one, two) = if team.one == team.two {
        (SlotId::One.to_string(), SlotId::Two.to_string())
    } else {
        (team.one.to_string(), team.two.to_string())
    };
    format!(
        "mix2-consult disagree <<'SPLIT'\n\
         {one}: cache the compiled schema in-process | chosen\n\
         {two}: move validation off the hot path | deferred\n\
         team: ship the cache now; file the validation rework as a follow-up\n\
         SPLIT"
    )
}

/// Parse a `mix2-consult disagree` payload into a [`DisagreementRecord`].
///
/// Grammar: one line per team slot, `<agent>: <position> | <outcome>`.
/// The slot ids `one:`/`two:` are always accepted; a harness name or display
/// name ("codex:", "Claude:") resolves only while exactly one slot runs that
/// harness (see [`Team::slot_named`]). Split on the LAST `|` since positions
/// may themselves contain pipes. A required `team: <resolution>` line
/// follows; lines after `team` that don't match an agent line fold into the
/// resolution, space-joined and hard-capped at 300 chars on a word boundary.
pub fn parse(text: &str, team: &Team) -> Result<DisagreementRecord, String> {
    let mut stances: Vec<Stance> = Vec::new();
    let mut resolution = String::new();
    let mut seen_team = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((name, rest)) = line.split_once(':') {
            let name_norm = name.trim().to_lowercase();
            if name_norm == "team" {
                seen_team = true;
                push_resolution_piece(&mut resolution, rest.trim());
                continue;
            }
            if let Some(slot) = team.slot_named(&name_norm) {
                stances.push(parse_stance_line(slot, team, rest)?);
                continue;
            }
        }

        if seen_team {
            push_resolution_piece(&mut resolution, line);
        }
    }

    for slot in SlotId::ALL {
        let count = stances.iter().filter(|s| s.slot == slot).count();
        if count != 1 {
            return Err("each agent needs exactly one line".to_string());
        }
    }

    for stance in &stances {
        if stance.position.is_empty() {
            return Err("position cannot be empty".to_string());
        }
        if stance.position.chars().count() > 200 {
            return Err("position too long — restate it in one line".to_string());
        }
    }

    let position_of = |slot: SlotId| {
        normalize_position(&stances.iter().find(|s| s.slot == slot).unwrap().position)
    };
    if position_of(SlotId::One) == position_of(SlotId::Two) {
        return Err(
            "both positions are the same — that's not a split; disclose the nuance in prose instead"
                .to_string(),
        );
    }

    if resolution.is_empty() {
        return Err("missing 'team: <resolution>' line".to_string());
    }

    Ok(DisagreementRecord {
        stances,
        resolution: cap_resolution(&resolution),
    })
}

/// Format a parse error as the refusal shown to the lead agent: the error,
/// a worked example that parses for this team, and a bounded-retry stop
/// condition.
pub fn refusal(err: &str, team: &Team) -> String {
    format!(
        "{err}\n\nExample:\n{}\n\nIf this fails twice, skip recording and state the disagreement in prose.",
        example_for(team)
    )
}

fn parse_stance_line(slot: SlotId, team: &Team, rest: &str) -> Result<Stance, String> {
    let (position_raw, outcome_raw) = rest.rsplit_once('|').ok_or_else(|| {
        format!(
            "{} line is missing '| <outcome>'",
            team.harness(slot).display_name()
        )
    })?;
    let outcome = parse_outcome(outcome_raw)?;
    Ok(Stance {
        slot,
        position: position_raw.trim().to_string(),
        outcome,
    })
}

fn parse_outcome(raw: &str) -> Result<Outcome, String> {
    let mut norm = raw.trim().to_lowercase();
    if norm.ends_with('.') {
        norm.pop();
    }
    let tail = norm.replace(' ', "-");
    match tail.as_str() {
        "chosen" | "shipped" => Ok(Outcome::Chosen),
        "deferred" | "follow-up" | "followup" => Ok(Outcome::Deferred),
        "dropped" | "set-aside" => Ok(Outcome::Dropped),
        _ => Err(format!(
            "unknown outcome '{tail}' — use chosen, deferred, or dropped"
        )),
    }
}

fn normalize_position(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn push_resolution_piece(resolution: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !resolution.is_empty() {
        resolution.push(' ');
    }
    resolution.push_str(piece);
}

/// Cap a resolution at 300 chars total, cutting on a word boundary and
/// appending `…` so the cap itself never lands mid-word.
fn cap_resolution(s: &str) -> String {
    const MAX_TOTAL: usize = 300;
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= MAX_TOTAL {
        return s.to_string();
    }
    let content_max = MAX_TOTAL - 1; // reserve one char for the ellipsis
    let mut cut = content_max;
    while cut > 0 && !chars[cut - 1].is_whitespace() {
        cut -= 1;
    }
    if cut == 0 {
        cut = content_max; // no earlier boundary — hard cut
    }
    let mut truncated: String = chars[..cut].iter().collect();
    while truncated.ends_with(char::is_whitespace) {
        truncated.pop();
    }
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::HarnessKind;

    fn team() -> Team {
        Team {
            one: HarnessKind::Claude,
            two: HarnessKind::Codex,
            lead: SlotId::One,
        }
    }

    #[test]
    fn parses_canonical_block() {
        let text = "claude: cache the compiled schema in-process | chosen\n\
                    codex: move validation off the hot path | deferred\n\
                    team: ship the cache now; file the rework as a follow-up";
        let r = parse(text, &team()).unwrap();
        assert_eq!(r.stances.len(), 2);
        assert_eq!(r.stances[0].slot, SlotId::One);
        assert_eq!(r.stances[0].outcome, Outcome::Chosen);
        assert_eq!(r.stances[1].slot, SlotId::Two);
        assert_eq!(r.stances[1].outcome, Outcome::Deferred);
        assert!(r.resolution.starts_with("ship the cache"));
    }

    #[test]
    fn accepts_slot_ids_as_canonical_names() {
        let text = "one: cache the compiled schema | chosen\n\
                    two: move validation off the hot path | deferred\n\
                    team: ship the cache";
        let r = parse(text, &team()).unwrap();
        assert_eq!(r.stances[0].slot, SlotId::One);
        assert_eq!(r.stances[1].slot, SlotId::Two);
    }

    #[test]
    fn harness_names_are_ambiguous_on_same_harness_teams() {
        let same = Team {
            one: HarnessKind::Codex,
            two: HarnessKind::Codex,
            lead: SlotId::One,
        };
        let by_name = "codex: a | chosen\ncodex: b | deferred\nteam: c";
        assert!(parse(by_name, &same).is_err(), "names cannot disambiguate");
        let by_slot = "one: a | chosen\ntwo: b | deferred\nteam: c";
        let r = parse(by_slot, &same).unwrap();
        assert_eq!(r.stances[0].slot, SlotId::One);
        assert_eq!(r.stances[1].slot, SlotId::Two);
    }

    #[test]
    fn splits_on_last_pipe_and_accepts_display_names() {
        let text = "Claude: use `string | null` in the schema | chosen\n\
                    Codex: keep the alias | dropped\n\
                    team: go with the union";
        let r = parse(text, &team()).unwrap();
        assert_eq!(r.stances[0].position, "use `string | null` in the schema");
    }

    #[test]
    fn accepts_outcome_synonyms() {
        let text = "claude: a | shipped\ncodex: b | follow-up\nteam: call";
        let r = parse(text, &team()).unwrap();
        assert_eq!(r.stances[0].outcome, Outcome::Chosen);
        assert_eq!(r.stances[1].outcome, Outcome::Deferred);
    }

    #[test]
    fn folds_extra_lines_into_resolution_with_cap() {
        let text = format!(
            "claude: a | chosen\ncodex: b | deferred\nteam: first.\n{}",
            "word ".repeat(100)
        );
        let r = parse(&text, &team()).unwrap();
        assert!(r.resolution.chars().count() <= 300);
        assert!(r.resolution.ends_with('…'));
    }

    #[test]
    fn rejects_missing_team_line() {
        assert!(parse("claude: a | chosen\ncodex: b | deferred", &team()).is_err());
    }

    #[test]
    fn rejects_unknown_outcome_naming_the_tail() {
        let err = parse("claude: a | maybe\ncodex: b | deferred\nteam: c", &team()).unwrap_err();
        assert!(err.contains("maybe"));
    }

    #[test]
    fn rejects_identical_positions() {
        let err = parse(
            "claude: Use the cache | chosen\ncodex: use  the cache | deferred\nteam: c",
            &team(),
        )
        .unwrap_err();
        assert!(err.contains("not a split"));
    }

    #[test]
    fn rejects_overlong_position() {
        let text = format!(
            "claude: {} | chosen\ncodex: b | deferred\nteam: c",
            "x".repeat(201)
        );
        assert!(parse(&text, &team()).unwrap_err().contains("too long"));
    }

    #[test]
    fn rejects_wrong_and_duplicate_agents() {
        assert!(parse("gemini: a | chosen\ncodex: b | deferred\nteam: c", &team()).is_err());
        assert!(parse("claude: a | chosen\nclaude: b | deferred\nteam: c", &team()).is_err());
    }

    #[test]
    fn example_constant_parses() {
        let body: String = DISAGREE_EXAMPLE
            .lines()
            .filter(|l| !l.starts_with("mix2-consult") && *l != "SPLIT")
            .collect::<Vec<_>>()
            .join("\n");
        parse(&body, &team()).unwrap();
    }

    #[test]
    fn examples_parse_for_their_team() {
        // A mixed team keeps the canonical harness-name example.
        assert_eq!(example_for(&team()), DISAGREE_EXAMPLE);

        // A same-harness team gets slot ids, and that form parses for it —
        // harness names would be ambiguous and refused.
        let same = Team {
            one: HarnessKind::Codex,
            two: HarnessKind::Codex,
            lead: SlotId::One,
        };
        let example = example_for(&same);
        assert!(example.contains("one: "));
        let body: String = example
            .lines()
            .filter(|l| !l.starts_with("mix2-consult") && *l != "SPLIT")
            .collect::<Vec<_>>()
            .join("\n");
        parse(&body, &same).unwrap();

        // The refusal embeds the team-appropriate example.
        assert!(refusal("bad", &same).contains("one: cache"));
    }
}
