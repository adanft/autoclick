use crate::config::{validate_target_template_name, AppConfig, ConfigLoadError, ConfigStore, RuleConfig};
use crate::monitor::MonitorSpec;
use anyhow::{anyhow, bail, Context, Result};
use std::io::{self, Write};
use std::path::Path;

const DEFAULT_INTERVAL_MS: u64 = 250;
const DEFAULT_MATCH_THRESHOLD: f32 = 0.95;

/// Abstraction used by tests to drive the interactive configuration flow.
pub(crate) trait PromptIo {
    fn prompt(&mut self, label: &str, default: Option<&str>) -> Result<String>;
}

pub(crate) struct ConsolePromptIo;

impl PromptIo for ConsolePromptIo {
    fn prompt(&mut self, label: &str, default: Option<&str>) -> Result<String> {
        let mut stdout = io::stdout();
        match default {
            Some(default) => write!(stdout, "{label} [{default}]: ")?,
            None => write!(stdout, "{label}: ")?,
        }
        stdout.flush()?;

        let mut buffer = String::new();
        io::stdin().read_line(&mut buffer)?;
        let trimmed = buffer.trim();

        if trimmed.is_empty() {
            if let Some(default) = default {
                return Ok(default.to_string());
            }
        }

        Ok(trimmed.to_string())
    }
}

/// Loads the saved configuration when possible, otherwise re-runs interactive setup.
pub(crate) fn load_or_configure_with_io(
    store: &ConfigStore,
    monitors: &[MonitorSpec],
    io: &mut impl PromptIo,
) -> Result<AppConfig> {
    if store.exists() {
        println!("found existing config at {}", store.path().display());
        if confirm(io, "reuse saved configuration? [Y/n] ", true)? {
            match store.load() {
                Ok(config) => return Ok(config),
                Err(ConfigLoadError::Incompatible(error)) => {
                    println!(
                        "saved config is incompatible with the current config schema; reconfiguration required"
                    );
                    println!("reason: {error}");
                }
                Err(ConfigLoadError::Io(error)) => {
                    return Err(error).context("failed to load saved configuration");
                }
            }
        }
    }

    let config = prompt_for_config(monitors, io)?;
    validate_template_assets_exist(&config.rules, &store.templates_dir())?;
    store.save(&config)?;
    Ok(config)
}

/// Interactively collects a complete application configuration.
pub(crate) fn prompt_for_config(
    monitors: &[MonitorSpec],
    io: &mut impl PromptIo,
) -> Result<AppConfig> {
    println!("select monitor:");
    for monitor in monitors {
        println!("{}", monitor.summary_line());
    }
    let monitor = select_monitor(monitors, &io.prompt("monitor index", Some("1"))?)?;

    let interval_default = DEFAULT_INTERVAL_MS.to_string();
    let interval_ms = io
        .prompt("scan interval in milliseconds", Some(&interval_default))?
        .parse::<u64>()
        .context("interval must be an integer")?;
    if interval_ms == 0 {
        bail!("interval must be greater than zero");
    }

    let threshold_default = DEFAULT_MATCH_THRESHOLD.to_string();
    let match_threshold = parse_match_threshold(
        &io.prompt("global match threshold (0.0-1.0)", Some(&threshold_default))?,
    )?;

    let rules = prompt_rules(io)?;

    Ok(AppConfig {
        monitor_name: monitor.name.clone(),
        interval_ms,
        match_threshold,
        rules,
    })
}

/// Parses and validates the configured OpenCV match threshold.
fn parse_match_threshold(raw: &str) -> Result<f32> {
    let threshold = raw
        .trim()
        .parse::<f32>()
        .context("match threshold must be a number")?;

    if !threshold.is_finite() {
        bail!("match threshold must be finite");
    }

    if !(0.0..=1.0).contains(&threshold) {
        bail!("match threshold must be between 0.0 and 1.0");
    }

    Ok(threshold)
}

/// Prompts for at least one template rule.
fn prompt_rules(io: &mut impl PromptIo) -> Result<Vec<RuleConfig>> {
    let mut rules = Vec::new();

    loop {
        let prompt_label = format!(
            "rule {}, target template (example.png)",
            rules.len() + 1
        );
        let target_template = io.prompt(&prompt_label, None)?;
        validate_target_template_name(&target_template)
            .context("rule target template is invalid")?;

        rules.push(RuleConfig {
            target_template: target_template.trim().to_string(),
        });

        if !confirm(io, "add another rule? [y/N] ", false)? {
            break;
        }
    }

    if rules.is_empty() {
        bail!("at least one rule is required");
    }

    Ok(rules)
}

fn validate_template_assets_exist(rules: &[RuleConfig], templates_dir: &Path) -> Result<()> {
    for rule in rules {
        let template_path = templates_dir.join(&rule.target_template);
        if !template_path.is_file() {
            bail!(
                "template asset `{}` was not found in {}",
                rule.target_template,
                templates_dir.display()
            );
        }
    }

    Ok(())
}

/// Resolves the numbered monitor selection entered by the user.
fn select_monitor<'a>(monitors: &'a [MonitorSpec], raw_selection: &str) -> Result<&'a MonitorSpec> {
    let selection = raw_selection
        .trim()
        .parse::<usize>()
        .context("monitor selection must be a number")?;

    monitors
        .iter()
        .find(|monitor| monitor.index == selection)
        .ok_or_else(|| anyhow!("monitor index `{selection}` is invalid"))
}

/// Reads a yes/no confirmation, falling back to the provided default on empty input.
fn confirm(io: &mut impl PromptIo, label: &str, default: bool) -> Result<bool> {
    let value = io.prompt(label, None)?;
    if value.trim().is_empty() {
        return Ok(default);
    }

    match value.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        other => bail!("unsupported confirmation response: {other}"),
    }
}
