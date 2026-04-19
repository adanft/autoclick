use crate::capture::CaptureService;
use crate::config::{AppConfig, RuleConfig};
use crate::matcher::{self, MatchSet, PreparedRule};
use crate::monitor::MonitorSpec;
use crate::rules::{self, PlannedClick};
use crate::ydotool::YdotoolManager;
use anyhow::{Context, Error, Result};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;
use tracing::{debug, info, warn};

#[derive(Debug)]
pub(crate) enum RuntimeCycleError {
    Capture(Error),
    Match(Error),
    Click(Error),
}

impl RuntimeCycleError {
    fn stage_label(&self) -> &'static str {
        match self {
            Self::Capture(_) => "capture",
            Self::Match(_) => "OpenCV match",
            Self::Click(_) => "click execution",
        }
    }
}

impl fmt::Display for RuntimeCycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (stage, error) = match self {
            Self::Capture(error) => ("capture", error),
            Self::Match(error) => ("OpenCV match", error),
            Self::Click(error) => ("click execution", error),
        };

        write!(f, "{stage} failed: {error:#}")
    }
}

/// Runs the background monitoring loop until the user requests shutdown.
pub fn run_monitor_loop(
    config: &AppConfig,
    prepared_rules: &[PreparedRule],
    monitor: &MonitorSpec,
    capture: &CaptureService,
    ydotool: &YdotoolManager,
    shutdown_rx: Receiver<()>,
) -> Result<()> {
    run_monitor_loop_with_runner(config.interval_ms, shutdown_rx, || {
        run_cycle(
            &config.rules,
            prepared_rules,
            config.match_threshold,
            monitor,
            capture,
            ydotool,
        )
        .map(|_| ())
    })
}

fn run_monitor_loop_with_runner<F>(
    interval_ms: u64,
    shutdown_rx: Receiver<()>,
    mut run_cycle: F,
) -> Result<()>
where
    F: FnMut() -> std::result::Result<(), RuntimeCycleError>,
{
    loop {
        if shutdown_rx.try_recv().is_ok() {
            println!("shutdown requested");
            break;
        }

        match run_cycle() {
            Ok(_) => {}
            Err(error) => {
                warn!(stage = error.stage_label(), error = %error, "cycle skipped after runtime failure");
            }
        }

        match shutdown_rx.recv_timeout(Duration::from_millis(interval_ms)) {
            Ok(_) => {
                println!("shutdown requested");
                break;
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

/// Executes one full runtime cycle: capture, match, evaluate rules, and click.
pub(crate) fn run_cycle(
    rules_config: &[RuleConfig],
    prepared_rules: &[PreparedRule],
    match_threshold: f32,
    monitor: &MonitorSpec,
    capture: &CaptureService,
    ydotool: &YdotoolManager,
) -> std::result::Result<Vec<PlannedClick>, RuntimeCycleError> {
    run_cycle_with(
        rules_config,
        prepared_rules,
        match_threshold,
        monitor,
        || capture.capture_monitor(monitor),
        |screenshot, threshold| {
            matcher::scan_all(screenshot, prepared_rules, threshold).with_context(|| {
                format!("OpenCV matching failed at threshold {:.2}", match_threshold)
            })
        },
        |matches| {
            execute_match_set(rules_config, monitor, matches, |x, y| {
                ydotool.execute_click(x, y)
            })
        },
    )
}

fn run_cycle_with<C, M, E>(
    rules_config: &[RuleConfig],
    prepared_rules: &[PreparedRule],
    match_threshold: f32,
    monitor: &MonitorSpec,
    capture_screenshot: C,
    scan_matches: M,
    execute_cycle: E,
) -> std::result::Result<Vec<PlannedClick>, RuntimeCycleError>
where
    C: FnOnce() -> Result<PathBuf>,
    M: FnOnce(&Path, f32) -> Result<MatchSet>,
    E: FnOnce(&MatchSet) -> Result<Vec<PlannedClick>>,
{
    let screenshot = capture_screenshot().map_err(RuntimeCycleError::Capture)?;
    debug!(monitor = %monitor.name, screenshot = %screenshot.display(), "captured screenshot");
    let matches = scan_matches(&screenshot, match_threshold).map_err(RuntimeCycleError::Match)?;
    log_match_diagnostics(rules_config, prepared_rules, &matches, match_threshold);
    execute_cycle(&matches).map_err(RuntimeCycleError::Click)
}

fn log_match_diagnostics(
    rules_config: &[RuleConfig],
    prepared_rules: &[PreparedRule],
    matches: &MatchSet,
    match_threshold: f32,
) {
    for (index, rule) in rules_config.iter().enumerate() {
        let template_size = prepared_rules
            .get(index)
            .map(|rule| format!("{}x{}", rule.template_size.0, rule.template_size.1))
            .unwrap_or_else(|| "unknown-size".to_string());

        match matches.get(&rule.target_template) {
            Some(regions) if !regions.is_empty() => {
                let first = &regions[0];
                debug!(
                    rule_index = index + 1,
                    target_template = %rule.target_template,
                    candidates = regions.len(),
                    threshold = match_threshold,
                    left = first.left,
                    top = first.top,
                    width = first.width,
                    height = first.height,
                    template_size = %template_size,
                    "rule matched template"
                );
            }
            _ => {
                debug!(
                    rule_index = index + 1,
                    target_template = %rule.target_template,
                    threshold = match_threshold,
                    template_size = %template_size,
                    "rule did not match template"
                );
            }
        }
    }
}

/// Converts accepted matches into click executions using the provided callback.
pub fn execute_match_set<F>(
    rules_config: &[RuleConfig],
    monitor: &MonitorSpec,
    matches: &MatchSet,
    mut execute_click: F,
) -> Result<Vec<PlannedClick>>
where
    F: FnMut(i32, i32) -> Result<()>,
{
    let planned = rules::evaluate_rules(rules_config, matches, monitor);
    for click in &planned {
        info!(
            rule_index = click.rule_index + 1,
            target_template = %click.target_template,
            x = click.abs_x,
            y = click.abs_y,
            "executing planned click"
        );
        execute_click(click.abs_x, click.abs_y)?;
        info!(rule_index = click.rule_index + 1, "click executed");
    }

    Ok(planned)
}
