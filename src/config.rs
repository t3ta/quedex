use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Configuration file name
const CONFIG_FILE_NAME: &str = "quedex.toml";

/// Configuration loaded from quedex.toml
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Maximum number of concurrent tasks
    pub max_concurrency: Option<usize>,
    /// Whether to stop on first failure
    pub fail_fast: Option<bool>,
    /// Path to store directory
    pub store: Option<PathBuf>,
    #[serde(
        rename = "timeout_sec",
        default,
        deserialize_with = "reject_timeout_sec",
        skip_serializing
    )]
    pub _timeout_sec_rejected: Option<serde::de::IgnoredAny>,
    #[serde(
        rename = "default_timeout_sec",
        default,
        deserialize_with = "reject_default_timeout_sec",
        skip_serializing
    )]
    pub _default_timeout_sec_rejected: Option<serde::de::IgnoredAny>,
    /// System prompt to prepend to all task prompts
    pub system_prompt: Option<String>,
}

fn reject_timeout_sec<'de, D>(deserializer: D) -> Result<Option<serde::de::IgnoredAny>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Err(serde::de::Error::custom(
        "timeout_sec は削除済みです。Codex CLI のタイムアウト設定を使用してください。",
    ))
}

fn reject_default_timeout_sec<'de, D>(
    deserializer: D,
) -> Result<Option<serde::de::IgnoredAny>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Err(serde::de::Error::custom(
        "default_timeout_sec は削除済みです。Codex CLI のタイムアウト設定を使用してください。",
    ))
}

impl Config {
    /// Load configuration from quedex.toml in the current directory.
    /// Returns default config if file doesn't exist.
    pub fn load() -> Result<Self> {
        Self::load_from_dir(Path::new("."))
    }

    /// Load configuration from quedex.toml in the specified directory.
    /// Returns default config if file doesn't exist.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let config_path = dir.join(CONFIG_FILE_NAME);
        Self::load_from_path(&config_path)
    }

    /// Load configuration from a specific path.
    /// Returns default config if file doesn't exist.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Config::default());
        }

        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;

        toml::from_str(&contents)
            .with_context(|| format!("failed to parse config file: {}", path.display()))
    }
}

/// Effective options after merging CLI options with config file.
/// CLI options take precedence over config file values.
#[derive(Debug, Clone)]
pub struct EffectiveOptions {
    pub max_concurrency: Option<usize>,
    pub fail_fast: bool,
    pub store: Option<PathBuf>,
}

impl EffectiveOptions {
    /// Create effective options by merging CLI options with config file.
    /// CLI options (if specified) take precedence over config file values.
    pub fn new(
        cli_store: Option<PathBuf>,
        cli_max_concurrency: Option<usize>,
        _cli_fail_fast: bool,
        cli_no_fail_fast: bool,
        config: &Config,
    ) -> Self {
        // store: CLI > config
        let store = cli_store.or_else(|| config.store.clone());

        // max_concurrency: CLI > config
        let max_concurrency = cli_max_concurrency.or(config.max_concurrency);

        // fail_fast: --no-fail-fast > config > default (true)
        // Note: cli_fail_fast defaults to true in CLI, so we can't distinguish
        // "user passed --fail-fast" from "default". Only --no-fail-fast is explicit.
        let fail_fast = if cli_no_fail_fast {
            false
        } else {
            config.fail_fast.unwrap_or(true)
        };

        Self {
            max_concurrency,
            fail_fast,
            store,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.max_concurrency.is_none());
        assert!(config.fail_fast.is_none());
        assert!(config.store.is_none());
        assert!(config.system_prompt.is_none());
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
max_concurrency = 4
fail_fast = false
store = "/tmp/quedex"
system_prompt = "You are a helpful assistant."
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.max_concurrency, Some(4));
        assert_eq!(config.fail_fast, Some(false));
        assert_eq!(config.store, Some(PathBuf::from("/tmp/quedex")));
        assert_eq!(
            config.system_prompt,
            Some("You are a helpful assistant.".to_string())
        );
    }

    #[test]
    fn test_parse_partial_config() {
        let toml = r#"
max_concurrency = 2
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.max_concurrency, Some(2));
        assert!(config.fail_fast.is_none());
        assert!(config.store.is_none());
    }

    #[test]
    fn test_effective_options_cli_precedence() {
        let config = Config {
            max_concurrency: Some(4),
            fail_fast: Some(false),
            store: Some(PathBuf::from("/config/store")),
            system_prompt: None,
            ..Default::default()
        };

        // CLI values should override config
        let effective = EffectiveOptions::new(
            Some(PathBuf::from("/cli/store")),
            Some(8),
            true,
            false,
            &config,
        );

        assert_eq!(effective.max_concurrency, Some(8));
        assert_eq!(effective.store, Some(PathBuf::from("/cli/store")));
        // fail_fast from config since CLI doesn't explicitly set it
        assert!(!effective.fail_fast);
    }

    #[test]
    fn test_effective_options_config_fallback() {
        let config = Config {
            max_concurrency: Some(4),
            fail_fast: Some(false),
            store: Some(PathBuf::from("/config/store")),
            system_prompt: None,
            ..Default::default()
        };

        // No CLI values, should use config
        let effective = EffectiveOptions::new(None, None, true, false, &config);

        assert_eq!(effective.max_concurrency, Some(4));
        assert_eq!(effective.store, Some(PathBuf::from("/config/store")));
        assert!(!effective.fail_fast);
    }

    #[test]
    fn test_effective_options_no_fail_fast() {
        let config = Config {
            max_concurrency: None,
            fail_fast: Some(true),
            store: None,
            system_prompt: None,
            ..Default::default()
        };

        // --no-fail-fast should override config
        let effective = EffectiveOptions::new(None, None, true, true, &config);

        assert!(!effective.fail_fast);
    }

    #[test]
    fn test_parse_multiline_system_prompt() {
        let toml = r#"
system_prompt = """
This project is written in Rust.
Coding conventions:
- Use snake_case
- Use anyhow for error handling
"""
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.system_prompt.is_some());
        let prompt = config.system_prompt.unwrap();
        assert!(prompt.contains("Rust"));
        assert!(prompt.contains("snake_case"));
    }
}
