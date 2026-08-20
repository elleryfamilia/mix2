use crate::agents::{HarnessKind, SlotId, Team};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// User configuration, loaded from `~/.config/mix2/config.toml`
/// (respecting `$XDG_CONFIG_HOME`). Everything is optional; precedence is
/// CLI > user config > defaults. Project-level config can slot in between
/// later without changing this shape.
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
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CollaborationConfig {
    pub max_consults_per_turn: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub command: Option<String>,
    /// Model override passed to the CLI; None uses the provider's default.
    pub model: Option<String>,
}

/// Fully-resolved settings for one team slot (its harness lives in `team`).
#[derive(Debug, Clone)]
pub struct SlotSettings {
    pub command: String,
    pub model: Option<String>,
}

/// Fully-resolved runtime configuration. The legacy config syntax names
/// harnesses, but it resolves to slots here: slot one is Claude, slot two is
/// Codex, and `lead = "codex"` picks slot two as lead. Everything downstream
/// keys on [`SlotId`].
#[derive(Debug, Clone)]
pub struct Config {
    pub team: Team,
    pub one: SlotSettings,
    pub two: SlotSettings,
    pub max_consults_per_turn: u32,
}

pub const DEFAULT_MAX_CONSULTS: u32 = 2;

impl Config {
    pub fn slot(&self, id: SlotId) -> &SlotSettings {
        match id {
            SlotId::One => &self.one,
            SlotId::Two => &self.two,
        }
    }

    /// Resolve from an optional CLI lead override plus a parsed config file.
    pub fn resolve(cli_lead: Option<&str>, file: &FileConfig) -> Result<Self> {
        let lead_str = cli_lead
            .map(str::to_owned)
            .or_else(|| file.lead.clone())
            .unwrap_or_else(|| "claude".to_owned());
        let lead_harness: HarnessKind = lead_str
            .parse()
            .map_err(|e: String| anyhow::anyhow!("invalid lead: {e}"))?;
        let team = Team {
            one: HarnessKind::Claude,
            two: HarnessKind::Codex,
            lead: match lead_harness {
                HarnessKind::Claude => SlotId::One,
                HarnessKind::Codex => SlotId::Two,
            },
        };
        Ok(Self {
            team,
            one: SlotSettings {
                command: file
                    .claude
                    .command
                    .clone()
                    .unwrap_or_else(|| "claude".to_owned()),
                model: file.claude.model.clone(),
            },
            two: SlotSettings {
                command: file
                    .codex
                    .command
                    .clone()
                    .unwrap_or_else(|| "codex".to_owned()),
                model: file.codex.model.clone(),
            },
            max_consults_per_turn: file
                .collaboration
                .max_consults_per_turn
                .unwrap_or(DEFAULT_MAX_CONSULTS),
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
    }

    #[test]
    fn file_lead_resolves_to_slot_two() {
        let cfg = Config::resolve(None, &parse("lead = \"codex\"")).unwrap();
        assert_eq!(cfg.team.lead, SlotId::Two);
        assert_eq!(cfg.team.lead_harness(), HarnessKind::Codex);
        assert_eq!(cfg.team.teammate_harness(), HarnessKind::Claude);
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
    }

    #[test]
    fn provider_command_override_lands_on_slots() {
        let cfg = Config::resolve(
            None,
            &parse("[claude]\ncommand = \"/custom/claude\"\n[codex]\ncommand = \"/custom/codex\""),
        )
        .unwrap();
        assert_eq!(cfg.slot(SlotId::One).command, "/custom/claude");
        assert_eq!(cfg.slot(SlotId::Two).command, "/custom/codex");
    }

    #[test]
    fn provider_model_override_lands_on_slots() {
        let cfg = Config::resolve(None, &parse("[claude]\nmodel = \"sonnet\"")).unwrap();
        assert_eq!(cfg.slot(SlotId::One).model.as_deref(), Some("sonnet"));
        assert_eq!(cfg.slot(SlotId::Two).model, None);
    }

    #[test]
    fn consult_budget_from_file() {
        let cfg =
            Config::resolve(None, &parse("[collaboration]\nmax_consults_per_turn = 1")).unwrap();
        assert_eq!(cfg.max_consults_per_turn, 1);
    }
}
