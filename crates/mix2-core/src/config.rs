use crate::agents::registry;
use crate::agents::{HarnessKind, SlotId, Team};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// User configuration, loaded from `~/.config/mix2/config.toml`
/// (respecting `$XDG_CONFIG_HOME`). Everything is optional; precedence is
/// CLI > user config > defaults.
///
/// Two schemas coexist and are never auto-migrated:
/// - canonical, slot-keyed: `lead = "one"` plus `[slot.one]`/`[slot.two]`
///   tables choosing a harness (and optional command/model) per slot;
/// - legacy, harness-keyed: `lead = "claude"` plus `[claude]`/`[codex]`
///   sections. An unchanged legacy file resolves exactly as it always has.
///
/// Per-slot precedence: slot values > the legacy section matching the
/// slot's harness > the registry's descriptor default.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub lead: Option<String>,
    #[serde(default)]
    pub collaboration: CollaborationConfig,
    #[serde(default)]
    pub claude: ProviderConfig,
    #[serde(default)]
    pub codex: ProviderConfig,
    #[serde(default)]
    pub slot: SlotTables,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CollaborationConfig {
    pub max_consults_per_turn: Option<u32>,
}

/// Legacy harness-keyed section (`[claude]` / `[codex]`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub command: Option<String>,
    /// Model override passed to the CLI; None uses the provider's default.
    pub model: Option<String>,
}

/// Canonical slot-keyed tables (`[slot.one]` / `[slot.two]`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SlotTables {
    pub one: Option<SlotEntry>,
    pub two: Option<SlotEntry>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SlotEntry {
    /// Which harness backs the slot; defaults to the slot's legacy pairing
    /// (one = claude, two = codex).
    pub harness: Option<String>,
    pub command: Option<String>,
    pub model: Option<String>,
}

/// Fully-resolved settings for one team slot (its harness lives in `team`).
#[derive(Debug, Clone)]
pub struct SlotSettings {
    pub command: String,
    pub model: Option<String>,
}

/// Fully-resolved runtime configuration. Everything downstream keys on
/// [`SlotId`]. `team` is the configured *proposal*; the session's actual
/// team is settled by the discovery/selection handshake.
#[derive(Debug, Clone)]
pub struct Config {
    pub team: Team,
    pub one: SlotSettings,
    pub two: SlotSettings,
    pub max_consults_per_turn: u32,
    /// Whether the file used the canonical `[slot.*]` schema — an explicit
    /// team choice that auto-confirms the selection handshake.
    pub explicit_slots: bool,
    /// Per-harness fallback command (legacy section > descriptor default)
    /// for harnesses picked onto a slot the config didn't assign them to.
    pub fallback_commands: Vec<(HarnessKind, String)>,
    /// Per-harness fallback model (the legacy section's, if any) for the
    /// same re-slotting case — models follow their harness like commands.
    pub fallback_models: Vec<(HarnessKind, Option<String>)>,
    /// Non-fatal configuration conflicts (a slot value shadowing a
    /// differing legacy value), surfaced as warning events at startup.
    pub warnings: Vec<String>,
}

pub const DEFAULT_MAX_CONSULTS: u32 = 2;

impl Config {
    pub fn slot(&self, id: SlotId) -> &SlotSettings {
        match id {
            SlotId::One => &self.one,
            SlotId::Two => &self.two,
        }
    }

    /// The command a harness runs with when no slot assignment ties it to
    /// more specific settings.
    pub fn fallback_command(&self, harness: HarnessKind) -> String {
        self.fallback_commands
            .iter()
            .find(|(h, _)| *h == harness)
            .map(|(_, c)| c.clone())
            .unwrap_or_else(|| registry::descriptor(harness).default_command.to_owned())
    }

    /// The command for `harness` when it is *selected* onto `slot`: the
    /// slot's configured command applies only while the selection matches
    /// the configured harness; otherwise the harness-level fallback.
    pub fn selection_command(&self, slot: SlotId, harness: HarnessKind) -> String {
        if self.team.harness(slot) == harness {
            self.slot(slot).command.clone()
        } else {
            self.fallback_command(harness)
        }
    }

    /// The model override a harness carries when no slot assignment ties it
    /// to more specific settings: its legacy section's model, if any.
    pub fn fallback_model(&self, harness: HarnessKind) -> Option<String> {
        self.fallback_models
            .iter()
            .find(|(h, _)| *h == harness)
            .and_then(|(_, m)| m.clone())
    }

    /// The model override for `harness` selected onto `slot` — configured
    /// models follow their harness, never the bare slot, mirroring
    /// [`Config::selection_command`].
    pub fn selection_model(&self, slot: SlotId, harness: HarnessKind) -> Option<String> {
        if self.team.harness(slot) == harness {
            self.slot(slot).model.clone()
        } else {
            self.fallback_model(harness)
        }
    }

    /// Resolve from an optional CLI lead override plus a parsed config file.
    pub fn resolve(cli_lead: Option<&str>, file: &FileConfig) -> Result<Self> {
        let mut warnings = Vec::new();

        let harness_for =
            |slot: SlotId, entry: Option<&SlotEntry>, default: HarnessKind| match entry
                .and_then(|e| e.harness.as_deref())
            {
                Some(name) => registry::harness_named(name).ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid [slot.{slot}] harness: {}",
                        registry::unknown_harness_message(name)
                    )
                }),
                None => Ok(default),
            };
        let one_harness = harness_for(SlotId::One, file.slot.one.as_ref(), HarnessKind::Claude)?;
        let two_harness = harness_for(SlotId::Two, file.slot.two.as_ref(), HarnessKind::Codex)?;

        let legacy_for = |harness: HarnessKind| match harness {
            HarnessKind::Claude => &file.claude,
            HarnessKind::Codex => &file.codex,
        };

        let mut settings_for = |slot: SlotId,
                                harness: HarnessKind,
                                entry: Option<&SlotEntry>|
         -> SlotSettings {
            let legacy = legacy_for(harness);
            let conflict = |slot_value: &Option<String>, legacy_value: &Option<String>| matches!((slot_value, legacy_value), (Some(s), Some(l)) if s != l);
            let slot_command = entry.and_then(|e| e.command.clone());
            if conflict(&slot_command, &legacy.command) {
                warnings.push(format!(
                    "config: [slot.{slot}] command overrides the [{harness}] command"
                ));
            }
            let slot_model = entry.and_then(|e| e.model.clone());
            if conflict(&slot_model, &legacy.model) {
                warnings.push(format!(
                    "config: [slot.{slot}] model overrides the [{harness}] model"
                ));
            }
            SlotSettings {
                command: slot_command
                    .or_else(|| legacy.command.clone())
                    .unwrap_or_else(|| registry::descriptor(harness).default_command.to_owned()),
                model: slot_model.or_else(|| legacy.model.clone()),
            }
        };
        let one = settings_for(SlotId::One, one_harness, file.slot.one.as_ref());
        let two = settings_for(SlotId::Two, two_harness, file.slot.two.as_ref());

        // Lead: `one`/`two` canonical; a harness name still works while
        // exactly one slot runs it. The default stays slot one (claude in
        // an unchanged legacy setup).
        let shape = Team {
            one: one_harness,
            two: two_harness,
            lead: SlotId::One,
        };
        let lead = match cli_lead.map(str::to_owned).or_else(|| file.lead.clone()) {
            None => SlotId::One,
            Some(name) => match shape.slot_named(&name) {
                Some(slot) => slot,
                None if registry::harness_named(&name).is_some() => anyhow::bail!(
                    "invalid lead: '{name}' does not name exactly one slot on this team — use 'one' or 'two'"
                ),
                None => anyhow::bail!(
                    "invalid lead: unknown slot '{name}' (expected 'one', 'two', or a harness name)"
                ),
            },
        };

        let fallback_commands = registry::ALL
            .into_iter()
            .map(|harness| {
                let command = legacy_for(harness)
                    .command
                    .clone()
                    .unwrap_or_else(|| registry::descriptor(harness).default_command.to_owned());
                (harness, command)
            })
            .collect();
        let fallback_models = registry::ALL
            .into_iter()
            .map(|harness| (harness, legacy_for(harness).model.clone()))
            .collect();

        Ok(Self {
            team: Team {
                one: one_harness,
                two: two_harness,
                lead,
            },
            one,
            two,
            max_consults_per_turn: file
                .collaboration
                .max_consults_per_turn
                .unwrap_or(DEFAULT_MAX_CONSULTS),
            explicit_slots: file.slot.one.is_some() || file.slot.two.is_some(),
            fallback_commands,
            fallback_models,
            warnings,
        })
    }
}

pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")));
    base.map(|b| b.join("mix2").join("config.toml"))
}

pub fn load_file(path: Option<&Path>) -> Result<FileConfig> {
    let path = match path {
        Some(p) => p.to_owned(),
        None => match config_path() {
            Some(p) => p,
            None => return Ok(FileConfig::default()),
        },
    };
    if !path.exists() {
        return Ok(FileConfig::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("invalid config {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> FileConfig {
        toml::from_str(s).expect("valid toml")
    }

    // ---------------------------------------------------------- legacy-only

    #[test]
    fn default_lead_is_slot_one_claude() {
        let cfg = Config::resolve(None, &FileConfig::default()).unwrap();
        assert_eq!(cfg.team.lead, SlotId::One);
        assert_eq!(cfg.team.one, HarnessKind::Claude);
        assert_eq!(cfg.team.two, HarnessKind::Codex);
        assert_eq!(cfg.team.teammate(), SlotId::Two);
        assert_eq!(cfg.max_consults_per_turn, DEFAULT_MAX_CONSULTS);
        assert_eq!(cfg.one.command, "claude");
        assert_eq!(cfg.two.command, "codex");
        assert!(cfg.warnings.is_empty());
    }

    #[test]
    fn legacy_file_resolves_identically() {
        let cfg = Config::resolve(
            None,
            &parse(
                "lead = \"codex\"\n[claude]\ncommand = \"/custom/claude\"\nmodel = \"sonnet\"\n[codex]\ncommand = \"/custom/codex\"",
            ),
        )
        .unwrap();
        assert_eq!(cfg.team.lead, SlotId::Two);
        assert_eq!(cfg.team.lead_harness(), HarnessKind::Codex);
        assert_eq!(cfg.slot(SlotId::One).command, "/custom/claude");
        assert_eq!(cfg.slot(SlotId::One).model.as_deref(), Some("sonnet"));
        assert_eq!(cfg.slot(SlotId::Two).command, "/custom/codex");
        assert!(cfg.warnings.is_empty());
    }

    #[test]
    fn cli_overrides_file() {
        let cfg = Config::resolve(Some("claude"), &parse("lead = \"codex\"")).unwrap();
        assert_eq!(cfg.team.lead, SlotId::One);
    }

    #[test]
    fn invalid_lead_rejected() {
        let err = Config::resolve(Some("gemini"), &FileConfig::default()).unwrap_err();
        assert!(err.to_string().contains("invalid lead"));
        assert!(err.to_string().contains("'one', 'two'"));
    }

    #[test]
    fn consult_budget_from_file() {
        let cfg =
            Config::resolve(None, &parse("[collaboration]\nmax_consults_per_turn = 1")).unwrap();
        assert_eq!(cfg.max_consults_per_turn, 1);
    }

    // ------------------------------------------------------------- new-only

    #[test]
    fn slot_schema_selects_harnesses_and_lead() {
        let cfg = Config::resolve(
            None,
            &parse(
                "lead = \"two\"\n[slot.one]\nharness = \"codex\"\ncommand = \"/x\"\n[slot.two]\nharness = \"claude\"\nmodel = \"opus\"",
            ),
        )
        .unwrap();
        assert_eq!(cfg.team.one, HarnessKind::Codex);
        assert_eq!(cfg.team.two, HarnessKind::Claude);
        assert_eq!(cfg.team.lead, SlotId::Two);
        assert_eq!(cfg.slot(SlotId::One).command, "/x");
        // No slot/legacy command: descriptor default for the slot's harness.
        assert_eq!(cfg.slot(SlotId::Two).command, "claude");
        assert_eq!(cfg.slot(SlotId::Two).model.as_deref(), Some("opus"));
        assert!(cfg.warnings.is_empty());
    }

    #[test]
    fn same_harness_team_is_expressible() {
        let cfg = Config::resolve(
            None,
            &parse(
                "lead = \"two\"\n[slot.one]\nharness = \"codex\"\n[slot.two]\nharness = \"codex\"",
            ),
        )
        .unwrap();
        assert_eq!(cfg.team.one, HarnessKind::Codex);
        assert_eq!(cfg.team.two, HarnessKind::Codex);
        assert_eq!(cfg.team.lead, SlotId::Two);
    }

    #[test]
    fn slot_entry_without_harness_keeps_the_legacy_pairing() {
        let cfg = Config::resolve(None, &parse("[slot.two]\nmodel = \"gpt-5\"")).unwrap();
        assert_eq!(cfg.team.two, HarnessKind::Codex);
        assert_eq!(cfg.slot(SlotId::Two).model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn unknown_slot_harness_is_a_registry_error() {
        let err = Config::resolve(None, &parse("[slot.one]\nharness = \"gemini\"")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("[slot.one]"), "{msg}");
        assert!(msg.contains("unknown harness 'gemini'"), "{msg}");
        assert!(msg.contains("claude, codex"), "{msg}");
    }

    // ---------------------------------------------------------------- mixed

    #[test]
    fn slot_values_shadow_legacy_with_a_warning() {
        let cfg = Config::resolve(
            None,
            &parse(
                "[claude]\ncommand = \"/legacy\"\nmodel = \"sonnet\"\n[slot.one]\ncommand = \"/slot\"\nmodel = \"opus\"",
            ),
        )
        .unwrap();
        assert_eq!(cfg.slot(SlotId::One).command, "/slot");
        assert_eq!(cfg.slot(SlotId::One).model.as_deref(), Some("opus"));
        assert_eq!(cfg.warnings.len(), 2);
        assert!(cfg.warnings[0].contains("[slot.one] command overrides"));
        assert!(cfg.warnings[1].contains("[slot.one] model overrides"));
    }

    #[test]
    fn identical_slot_and_legacy_values_do_not_warn() {
        let cfg = Config::resolve(
            None,
            &parse("[claude]\ncommand = \"/same\"\n[slot.one]\ncommand = \"/same\""),
        )
        .unwrap();
        assert!(cfg.warnings.is_empty());
    }

    #[test]
    fn legacy_section_matches_by_harness_not_by_slot() {
        // Slot one runs codex, so the [codex] section feeds it.
        let cfg = Config::resolve(
            None,
            &parse("[codex]\ncommand = \"/cc\"\n[slot.one]\nharness = \"codex\""),
        )
        .unwrap();
        assert_eq!(cfg.slot(SlotId::One).command, "/cc");
    }

    // ---------------------------------------------------- lead disambiguation

    #[test]
    fn legacy_lead_name_resolves_only_when_unambiguous() {
        let cfg = Config::resolve(None, &parse("lead = \"codex\"")).unwrap();
        assert_eq!(cfg.team.lead, SlotId::Two);

        let err = Config::resolve(
            Some("codex"),
            &parse("[slot.one]\nharness = \"codex\"\n[slot.two]\nharness = \"codex\""),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("does not name exactly one slot"), "{msg}");
        assert!(msg.contains("use 'one' or 'two'"), "{msg}");
    }

    #[test]
    fn selection_settings_follow_the_harness_when_reslotted() {
        // Slot one is configured for claude; the picker moves codex onto it.
        // Codex's legacy command AND model follow it to the new slot; the
        // claude-specific slot settings do not leak across harnesses.
        let cfg = Config::resolve(
            None,
            &parse("[claude]\nmodel = \"sonnet\"\n[codex]\ncommand = \"/cc\"\nmodel = \"gpt-5\""),
        )
        .unwrap();
        assert_eq!(
            cfg.selection_command(SlotId::One, HarnessKind::Codex),
            "/cc"
        );
        assert_eq!(
            cfg.selection_model(SlotId::One, HarnessKind::Codex)
                .as_deref(),
            Some("gpt-5")
        );
        // Matching harness still prefers the slot-resolved settings.
        assert_eq!(
            cfg.selection_model(SlotId::One, HarnessKind::Claude)
                .as_deref(),
            Some("sonnet")
        );
    }

    #[test]
    fn canonical_lead_slots_always_resolve() {
        let cfg = Config::resolve(
            Some("two"),
            &parse("[slot.one]\nharness = \"codex\"\n[slot.two]\nharness = \"codex\""),
        )
        .unwrap();
        assert_eq!(cfg.team.lead, SlotId::Two);
    }
}
