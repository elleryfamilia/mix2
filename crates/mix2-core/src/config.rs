use crate::agents::AgentKind;
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

/// Fully-resolved runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub lead: AgentKind,
    /// True when the user chose the lead (CLI flag or config file); false
    /// when it's the built-in default and may auto-fall-back.
    pub lead_explicit: bool,
    pub teammate: AgentKind,
    pub max_consults_per_turn: u32,
    pub claude_command: String,
    pub codex_command: String,
    pub claude_model: Option<String>,
    pub codex_model: Option<String>,
}

pub const DEFAULT_MAX_CONSULTS: u32 = 2;

impl Config {
    pub fn command_for(&self, kind: AgentKind) -> &str {
        match kind {
            AgentKind::Claude => &self.claude_command,
            AgentKind::Codex => &self.codex_command,
        }
    }

    /// Resolve from an optional CLI lead override plus a parsed config file.
    pub fn resolve(cli_lead: Option<&str>, file: &FileConfig) -> Result<Self> {
        let explicit = cli_lead.map(str::to_owned).or_else(|| file.lead.clone());
        let lead_explicit = explicit.is_some();
        let lead_str = explicit.unwrap_or_else(|| "claude".to_owned());
        let lead: AgentKind = lead_str
            .parse()
            .map_err(|e: String| anyhow::anyhow!("invalid lead: {e}"))?;
        Ok(Self {
            lead,
            lead_explicit,
            teammate: lead.other(),
            max_consults_per_turn: file
                .collaboration
                .max_consults_per_turn
                .unwrap_or(DEFAULT_MAX_CONSULTS),
            claude_command: file
                .claude
                .command
                .clone()
                .unwrap_or_else(|| "claude".to_owned()),
            codex_command: file
                .codex
                .command
                .clone()
                .unwrap_or_else(|| "codex".to_owned()),
            claude_model: file.claude.model.clone(),
            codex_model: file.codex.model.clone(),
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
    fn default_lead_is_claude() {
        let cfg = Config::resolve(None, &FileConfig::default()).unwrap();
        assert_eq!(cfg.lead, AgentKind::Claude);
        assert!(!cfg.lead_explicit, "the built-in default is not explicit");
        assert_eq!(cfg.teammate, AgentKind::Codex);
        assert_eq!(cfg.max_consults_per_turn, DEFAULT_MAX_CONSULTS);
        assert_eq!(cfg.claude_command, "claude");
        assert_eq!(cfg.codex_command, "codex");
    }

    #[test]
    fn file_lead_applies() {
        let cfg = Config::resolve(None, &parse("lead = \"codex\"")).unwrap();
        assert_eq!(cfg.lead, AgentKind::Codex);
        assert_eq!(cfg.teammate, AgentKind::Claude);
    }

    #[test]
    fn cli_overrides_file() {
        let cfg = Config::resolve(Some("claude"), &parse("lead = \"codex\"")).unwrap();
        assert_eq!(cfg.lead, AgentKind::Claude);
        assert!(cfg.lead_explicit);
    }

    #[test]
    fn invalid_lead_rejected() {
        let err = Config::resolve(Some("gemini"), &FileConfig::default()).unwrap_err();
        assert!(err.to_string().contains("invalid lead"));
    }

    #[test]
    fn provider_command_override() {
        let cfg = Config::resolve(
            None,
            &parse("[claude]\ncommand = \"/custom/claude\"\n[codex]\ncommand = \"/custom/codex\""),
        )
        .unwrap();
        assert_eq!(cfg.claude_command, "/custom/claude");
        assert_eq!(cfg.codex_command, "/custom/codex");
    }

    #[test]
    fn provider_model_override() {
        let cfg = Config::resolve(None, &parse("[claude]\nmodel = \"sonnet\"")).unwrap();
        assert_eq!(cfg.claude_model.as_deref(), Some("sonnet"));
        assert_eq!(cfg.codex_model, None);
    }

    #[test]
    fn consult_budget_from_file() {
        let cfg =
            Config::resolve(None, &parse("[collaboration]\nmax_consults_per_turn = 1")).unwrap();
        assert_eq!(cfg.max_consults_per_turn, 1);
    }
}
