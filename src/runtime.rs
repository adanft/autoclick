use crate::capture::{CaptureService, CapturedImage};
use crate::config::{AppConfig, RuleConfig};
use crate::matcher::{self, MatchSet, PreparedRule};
use crate::monitor::MonitorSpec;
use crate::rules;
use crate::wayland_pointer::{ClickExecutor, ImageExtent, PlannedClick};
use anyhow::{Context, Error, Result};
use opencv::core::Mat;
use std::fmt;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
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

impl std::error::Error for RuntimeCycleError {}

/// Runs the background monitoring loop until the user requests shutdown.
pub fn run_monitor_loop(
    config: &AppConfig,
    prepared_rules: &[PreparedRule],
    monitor: &MonitorSpec,
    capture: &CaptureService,
    executor: &mut impl ClickExecutor,
    shutdown_rx: Receiver<()>,
) -> Result<()> {
    run_monitor_loop_with_runner(config.interval_ms, shutdown_rx, || {
        run_cycle(
            &config.rules,
            prepared_rules,
            config.match_threshold,
            monitor,
            capture,
            executor,
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
    let interval = Duration::from_millis(interval_ms);

    loop {
        if shutdown_rx.try_recv().is_ok() {
            println!("shutdown requested");
            break;
        }

        let cycle_started = Instant::now();
        match run_cycle() {
            Ok(_) => {}
            Err(error) => {
                if matches!(error, RuntimeCycleError::Click(_)) {
                    return Err(error)
                        .context("monitor loop stopped because click injection failed");
                }
                warn!(stage = error.stage_label(), error = %error, "cycle skipped after runtime failure");
            }
        }

        // Wait out only what is left of the interval. Sleeping the full interval
        // after the cycle would make the real scan period `interval_ms` plus the
        // capture and match time, which drifts further apart the slower a cycle is.
        let remaining = interval.saturating_sub(cycle_started.elapsed());
        match shutdown_rx.recv_timeout(remaining) {
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
    executor: &mut impl ClickExecutor,
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
        |matches, extent| execute_match_set(rules_config, extent, matches, executor),
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
    C: FnOnce() -> Result<CapturedImage>,
    M: FnOnce(&Mat, f32) -> Result<MatchSet>,
    E: FnOnce(&MatchSet, ImageExtent) -> Result<Vec<PlannedClick>>,
{
    let screenshot = capture_screenshot().map_err(RuntimeCycleError::Capture)?;
    debug!(monitor = %monitor.name, screenshot = %screenshot.path.display(), "captured screenshot");
    let matches =
        scan_matches(&screenshot.image, match_threshold).map_err(RuntimeCycleError::Match)?;
    log_match_diagnostics(rules_config, prepared_rules, &matches, match_threshold);
    execute_cycle(&matches, screenshot.extent).map_err(RuntimeCycleError::Click)
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

/// Converts accepted matches into Wayland click executions.
pub fn execute_match_set(
    rules_config: &[RuleConfig],
    extent: ImageExtent,
    matches: &MatchSet,
    executor: &mut impl ClickExecutor,
) -> Result<Vec<PlannedClick>> {
    let planned = rules::evaluate_rules(rules_config, matches, extent);
    for click in &planned {
        info!(
            rule_index = click.rule_index + 1,
            target_template = %click.target_template,
            output_x = click.output_x,
            output_y = click.output_y,
            "executing planned Wayland click"
        );
        executor
            .click(click)
            .context("Wayland virtual-pointer click failed")?;
        info!(rule_index = click.rule_index + 1, target_template = %click.target_template, "Wayland click executed");
    }
    Ok(planned)
}
