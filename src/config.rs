use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Component;
use std::path::{Path, PathBuf};

/// One template-matching rule stored in the persisted configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleConfig {
    pub target_template: String,
}

/// Persisted runtime configuration loaded from `config.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    pub monitor_name: String,
    pub interval_ms: u64,
    pub match_threshold: f32,
    pub rules: Vec<RuleConfig>,
}

/// Locates and persists the application configuration file.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

/// Distinguishes plain I/O failures from schema incompatibilities.
#[derive(Debug)]
pub enum ConfigLoadError {
    Io(anyhow::Error),
    Incompatible(anyhow::Error),
}

impl fmt::Display for ConfigLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) | Self::Incompatible(error) => write!(f, "{error}"),
        }
    }
}

impl Error for ConfigLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) | Self::Incompatible(error) => Some(error.root_cause()),
        }
    }
}

impl ConfigStore {
    /// Creates a config store using the default XDG-aware config path.
    pub fn new() -> Result<Self> {
        Ok(Self {
            path: resolve_config_path()?,
        })
    }

    /// Creates a config store for an explicit path, mainly for tests.
    pub fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns the full config file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the sibling `templates/` directory used by rule assets.
    pub fn templates_dir(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("templates")
    }

    /// Returns `true` when the config file already exists on disk.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Loads and validates the persisted configuration.
    pub fn load(&self) -> std::result::Result<AppConfig, ConfigLoadError> {
        let raw = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read config file at {}", self.path.display()))
            .map_err(ConfigLoadError::Io)?;
        parse_config(&raw).map_err(ConfigLoadError::Incompatible)
    }

    /// Validates and saves the configuration as pretty JSON.
    pub fn save(&self, config: &AppConfig) -> Result<()> {
        validate_app_config(config)?;

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory at {}", parent.display())
            })?;
        }

        let content = serde_json::to_string_pretty(config)?;
        fs::write(&self.path, content)
            .with_context(|| format!("failed to write config file at {}", self.path.display()))?;
        Ok(())
    }
}

/// Parses and validates a serialized configuration payload.
pub fn parse_config(raw: &str) -> Result<AppConfig> {
    let value: Value = serde_json::from_str(raw).context("config is not valid JSON")?;
    validate_config_schema(&value)?;
    let config = serde_json::from_value::<AppConfig>(value).context("config schema is invalid")?;

    validate_app_config(&config)?;

    Ok(config)
}

fn validate_app_config(config: &AppConfig) -> Result<()> {
    validate_match_threshold(config.match_threshold)?;

    if config.monitor_name.trim().is_empty() {
        bail!("config.monitor_name cannot be empty");
    }

    if config.interval_ms == 0 {
        bail!("config.interval_ms must be greater than zero");
    }

    if config.rules.is_empty() {
        bail!("config.rules must include at least one rule");
    }

    for (index, rule) in config.rules.iter().enumerate() {
        validate_target_template_name(&rule.target_template)
            .with_context(|| format!("config.rules[{index}].target_template is invalid"))?;
    }

    Ok(())
}

pub(crate) fn validate_target_template_name(target_template: &str) -> Result<()> {
    let trimmed = target_template.trim();
    if trimmed.is_empty() {
        bail!("target template cannot be empty");
    }

    let path = Path::new(trimmed);
    if path.is_absolute() {
        bail!("target template must be a filename inside templates/");
    }

    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("target template must not include path segments");
    }

    Ok(())
}

fn validate_config_schema(value: &Value) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("config root must be a JSON object"))?;

    let allowed: BTreeSet<&str> = ["monitor_name", "interval_ms", "match_threshold", "rules"]
        .into_iter()
        .collect();

    if object.contains_key("version") {
        bail!("config schema must not include a version field in v1");
    }

    for key in object.keys() {
        if !allowed.contains(key.as_str()) {
            bail!("unsupported config field: {key}");
        }
    }

    let match_threshold = object
        .get("match_threshold")
        .ok_or_else(|| anyhow!("config.match_threshold is required"))?;

    if !match_threshold.is_number() {
        bail!("config.match_threshold must be a number");
    }

    let rules = object
        .get("rules")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("config.rules must be an array"))?;

    for (index, rule) in rules.iter().enumerate() {
        let rule_object = rule
            .as_object()
            .ok_or_else(|| anyhow!("config.rules[{index}] must be an object"))?;

        if rule_object.contains_key("context") || rule_object.contains_key("target") {
            bail!(
                "legacy rule schema at config.rules[{index}] is incompatible; reconfigure with target_template"
            );
        }

        let allowed_rule_fields: BTreeSet<&str> = ["target_template"].into_iter().collect();
        for key in rule_object.keys() {
            if !allowed_rule_fields.contains(key.as_str()) {
                bail!("unsupported rule field at config.rules[{index}]: {key}");
            }
        }
    }

    Ok(())
}

fn validate_match_threshold(threshold: f32) -> Result<()> {
    if !threshold.is_finite() {
        bail!("config.match_threshold must be finite");
    }

    if !(0.0..=1.0).contains(&threshold) {
        bail!("config.match_threshold must be between 0.0 and 1.0");
    }

    Ok(())
}

fn resolve_config_path() -> Result<PathBuf> {
    if let Ok(explicit_path) = env::var("AUTOCLICK_CONFIG_PATH") {
        return Ok(PathBuf::from(explicit_path));
    }

    if let Ok(xdg_config_home) = env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg_config_home)
            .join("autoclick")
            .join("config.json"));
    }

    let home = env::var("HOME").context("HOME is not set and XDG_CONFIG_HOME is unavailable")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("autoclick")
        .join("config.json"))
}
