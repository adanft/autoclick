use crate::capture::{CaptureService, CapturedImage, Disconnect, FrameSource};
use crate::config::{AppConfig, RuleConfig};
use crate::matcher::{self, ColorRejections, MatchSet, PreparedRule, TemplateScores};
use crate::monitor::MonitorSpec;
use crate::rules;
use crate::wayland_pointer::{ClickExecutor, ImageExtent, PlannedClick};
use anyhow::{anyhow, Context, Error, Result};
use opencv::core::Mat;
use opencv::prelude::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Consecutive capture or match failures after which the loop stops scanning
/// at its interval and waits for the output to recover instead.
///
/// A single bad frame recovers on the next cycle, but a compositor that keeps
/// failing frames, such as one whose output is powered off, or keeps rejecting
/// this client fails every cycle. Five in a row rides out a brief stall; past
/// that, retrying at the scan interval would only flood the log, so the loop
/// enters the same wait a disconnect enters at once. It never exits over them.
const MAX_CONSECUTIVE_CYCLE_FAILURES: usize = 5;

/// First wait before an attempt to recover a failing output; every failed
/// attempt doubles it, up to [`RECONNECT_MAX_DELAY`].
///
/// Unplugging and replugging a monitor takes the compositor around a second
/// to settle, so trying sooner would only add failed attempts.
const RECONNECT_FIRST_DELAY: Duration = Duration::from_secs(1);

/// Longest wait between two recovery attempts, so an output that comes back
/// after a long absence, or a screen that powers on again, is picked up within
/// ten seconds, and a failure that repeats on every attempt, such as a protocol
/// error, is retried no more often than this.
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(10);

/// How often a still-failing output is reported again while waiting for it.
const RECONNECT_REMINDER_INTERVAL: Duration = Duration::from_secs(60);

/// Consecutive cycles a template must stay unclickable, found in grayscale but
/// rejected by the color check or too large to be scored, before a warning.
///
/// One minute at a five-second interval: long enough that a button merely
/// dimmed while it animates in is never reported, short enough that a template
/// the screen can never satisfy does not fail silently for hours.
const UNCLICKABLE_STREAK_CYCLES: u32 = 12;

/// Shortest time between two warnings about the same unclickable template.
const UNCLICKABLE_WARNING_INTERVAL: Duration = Duration::from_secs(60);

/// Timing of the wait for a failing output to recover.
#[derive(Debug, Clone, Copy)]
struct ReconnectBackoff {
    first_delay: Duration,
    max_delay: Duration,
    reminder_interval: Duration,
}

impl ReconnectBackoff {
    const STANDARD: Self = Self {
        first_delay: RECONNECT_FIRST_DELAY,
        max_delay: RECONNECT_MAX_DELAY,
        reminder_interval: RECONNECT_REMINDER_INTERVAL,
    };

    /// The wait after a failed attempt that followed a wait of `delay`.
    fn next_delay(&self, delay: Duration) -> Duration {
        delay.saturating_mul(2).min(self.max_delay)
    }
}

/// How one wait before a recovery attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconnectOutcome {
    /// New clients are connected; a cycle on them decides whether the output
    /// recovered.
    Reconnected,
    Shutdown,
}

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

    /// The disconnect behind a failed capture or click, if there is one.
    fn disconnect(&self) -> Option<&Disconnect> {
        match self {
            Self::Capture(error) | Self::Click(error) => Disconnect::find(error),
            Self::Match(_) => None,
        }
    }

    /// Whether the failure ends the loop: a click that failed for any reason
    /// but a disconnect or a stall, such as a rotated output or a lost seat or
    /// manager. Those are configuration drift that no wait repairs.
    fn is_fatal(&self) -> bool {
        matches!(self, Self::Click(_)) && self.disconnect().is_none()
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
///
/// `backends` are the capture service and click executor connected at
/// startup. When either reports a [`Disconnect`], or cycles keep failing, the
/// loop waits for the output: both are dropped and `connect` is retried for the
/// monitor's connector, with backoff, until a cycle on a fresh pair succeeds or
/// shutdown is requested.
pub fn run_monitor_loop<S, E, F>(
    config: &AppConfig,
    prepared_rules: &[PreparedRule],
    monitor: &MonitorSpec,
    backends: (CaptureService<S>, E),
    connect: F,
    shutdown_rx: Receiver<()>,
) -> Result<()>
where
    S: FrameSource,
    E: ClickExecutor,
    F: FnMut(&str) -> Result<(CaptureService<S>, E)>,
{
    run_monitor_loop_with_backoff(
        config,
        prepared_rules,
        monitor,
        backends,
        connect,
        ReconnectBackoff::STANDARD,
        shutdown_rx,
    )
}

/// [`run_monitor_loop`] with the recovery timing as a test seam.
fn run_monitor_loop_with_backoff<S, E, F>(
    config: &AppConfig,
    prepared_rules: &[PreparedRule],
    monitor: &MonitorSpec,
    backends: (CaptureService<S>, E),
    connect: F,
    backoff: ReconnectBackoff,
    shutdown_rx: Receiver<()>,
) -> Result<()>
where
    S: FrameSource,
    E: ClickExecutor,
    F: FnMut(&str) -> Result<(CaptureService<S>, E)>,
{
    let cycles = WaylandCycles {
        config,
        prepared_rules,
        monitor,
        backends: Some(backends),
        connect,
        history: CycleHistory::default(),
    };
    run_monitor_loop_with_runner(
        &monitor.name,
        config.interval_ms,
        backoff,
        shutdown_rx,
        cycles,
    )
}

/// What the monitor loop drives: one cycle at a time, and the replacement of
/// the clients behind them. Kept apart from the loop's timing and failure
/// policy.
trait MonitorCycles {
    fn run_cycle(&mut self) -> std::result::Result<(), RuntimeCycleError>;

    /// Drops the clients behind the cycles and connects new ones.
    fn reconnect(&mut self) -> Result<()>;
}

/// A bare cycle function owns no clients, so there is nothing to rebuild.
impl<F> MonitorCycles for F
where
    F: FnMut() -> std::result::Result<(), RuntimeCycleError>,
{
    fn run_cycle(&mut self) -> std::result::Result<(), RuntimeCycleError> {
        self()
    }

    fn reconnect(&mut self) -> Result<()> {
        Ok(())
    }
}

/// The real cycles: capture and click through Wayland clients that are
/// rebuilt, both together, while the loop waits for the output.
struct WaylandCycles<'a, S, E, F> {
    config: &'a AppConfig,
    prepared_rules: &'a [PreparedRule],
    monitor: &'a MonitorSpec,
    /// `None` only after a reconnect attempt failed.
    backends: Option<(CaptureService<S>, E)>,
    connect: F,
    history: CycleHistory,
}

impl<S, E, F> MonitorCycles for WaylandCycles<'_, S, E, F>
where
    S: FrameSource,
    E: ClickExecutor,
    F: FnMut(&str) -> Result<(CaptureService<S>, E)>,
{
    fn run_cycle(&mut self) -> std::result::Result<(), RuntimeCycleError> {
        let Some((capture, executor)) = self.backends.as_mut() else {
            return Err(RuntimeCycleError::Capture(anyhow!(
                "output {} has no connected Wayland clients",
                self.monitor.name
            )));
        };
        run_cycle(
            &self.config.rules,
            self.prepared_rules,
            self.config.match_threshold,
            self.monitor,
            capture,
            executor,
            &mut self.history,
        )
        .map(|_| ())
    }

    fn reconnect(&mut self) -> Result<()> {
        // Neither client can follow its output back: a returning output is a
        // new `wl_output` global, and a failed or stalled connection stays
        // that way. Both go before new ones connect.
        self.backends = None;
        // A reconnected output shows a new screen: turns start over.
        self.history.previous_click = None;
        self.backends = Some((self.connect)(&self.monitor.name)?);
        Ok(())
    }
}

/// What one cycle leaves for the next.
#[derive(Default)]
pub(crate) struct CycleHistory {
    /// The rule clicked in the previous cycle, so the next cycle can give the
    /// other matched rules a turn first. Starts empty: no cycle has clicked yet.
    previous_click: Option<usize>,
    unclickable: UnclickableStreaks,
}

/// The wait for a failing output: entered on a [`Disconnect`] or after
/// [`MAX_CONSECUTIVE_CYCLE_FAILURES`] failed cycles, and left only by a
/// successful cycle or by shutdown.
///
/// Every attempt drops both Wayland clients, connects new ones, and runs one
/// cycle on them. Each failed attempt, whether connecting or that cycle failed,
/// doubles the next wait up to the cap. A reconnect alone does not reset it, so
/// a client the compositor keeps rejecting is retried no faster than the cap.
struct Waiting {
    since: Instant,
    last_report: Instant,
    delay: Duration,
    attempt: u64,
    last_error: String,
}

impl Waiting {
    fn new(cause: &RuntimeCycleError, backoff: &ReconnectBackoff) -> Self {
        let now = Instant::now();
        Self {
            since: now,
            last_report: now,
            delay: backoff.first_delay,
            attempt: 0,
            last_error: cause.to_string(),
        }
    }

    /// Waits out the current delay, then drops and reconnects the clients,
    /// until a reconnect succeeds or shutdown is requested. Every wait listens
    /// on `shutdown_rx`, so a shutdown request ends it at once.
    fn reconnect(
        &mut self,
        connector: &str,
        backoff: &ReconnectBackoff,
        shutdown_rx: &Receiver<()>,
        cycles: &mut impl MonitorCycles,
    ) -> ReconnectOutcome {
        loop {
            if shutdown_within(shutdown_rx, self.delay) {
                return ReconnectOutcome::Shutdown;
            }
            self.attempt += 1;
            match cycles.reconnect() {
                Ok(()) => {
                    debug!(
                        attempt = self.attempt,
                        "output {connector} reconnected; checking it with a cycle"
                    );
                    return ReconnectOutcome::Reconnected;
                }
                Err(error) => self.attempt_failed(connector, format!("{error:#}"), backoff),
            }
        }
    }

    /// Records a failed attempt: doubles the next wait, and repeats the
    /// warning, with the latest cause, once per reminder interval.
    fn attempt_failed(&mut self, connector: &str, error: String, backoff: &ReconnectBackoff) {
        debug!(attempt = self.attempt, error = %error, "recovery attempt failed");
        self.last_error = error;
        self.delay = backoff.next_delay(self.delay);
        if self.last_report.elapsed() >= backoff.reminder_interval {
            self.last_report = Instant::now();
            warn!(
                waited_s = self.since.elapsed().as_secs(),
                attempts = self.attempt,
                last_error = %self.last_error,
                "output {connector} is still unavailable; waiting for it to return"
            );
        }
    }

    /// Ends the wait after a successful cycle.
    ///
    /// Logged at `warn`, like the message that opened the wait, so the default
    /// filter shows when an outage ended and not only when it began.
    fn resumed(self, connector: &str) {
        warn!(
            attempts = self.attempt,
            waited_s = self.since.elapsed().as_secs(),
            "monitoring of output {connector} resumed"
        );
    }
}

/// Waits up to `timeout` for a shutdown request, returning whether one came.
/// A closed channel counts as one: nobody is left to ask for it.
fn shutdown_within(shutdown_rx: &Receiver<()>, timeout: Duration) -> bool {
    match shutdown_rx.recv_timeout(timeout) {
        Ok(()) => {
            println!("shutdown requested");
            true
        }
        Err(RecvTimeoutError::Timeout) => false,
        Err(RecvTimeoutError::Disconnected) => true,
    }
}

/// Returns the rule a cycle clicked, which the next cycle searches after.
///
/// A cycle that clicked nothing, or failed before clicking, resets the turn
/// order so the next click follows configuration order again.
fn rule_clicked_by(
    outcome: &std::result::Result<Option<PlannedClick>, RuntimeCycleError>,
) -> Option<usize> {
    match outcome {
        Ok(Some(click)) => Some(click.rule_index),
        Ok(None) | Err(_) => None,
    }
}

/// Runs cycles every `interval_ms` until shutdown.
///
/// Only shutdown and a fatal click failure (see [`RuntimeCycleError::is_fatal`])
/// end it. A disconnect, or [`MAX_CONSECUTIVE_CYCLE_FAILURES`] other failed
/// cycles in a row, enter a [`Waiting`] state instead, which only a successful
/// cycle leaves.
fn run_monitor_loop_with_runner(
    connector: &str,
    interval_ms: u64,
    backoff: ReconnectBackoff,
    shutdown_rx: Receiver<()>,
    mut cycles: impl MonitorCycles,
) -> Result<()> {
    let interval = Duration::from_millis(interval_ms);
    let mut consecutive_failures = 0_usize;
    let mut waiting: Option<Waiting> = None;

    loop {
        if shutdown_rx.try_recv().is_ok() {
            println!("shutdown requested");
            break;
        }

        let cycle_started = Instant::now();
        match cycles.run_cycle() {
            Ok(()) => {
                consecutive_failures = 0;
                if let Some(waiting) = waiting.take() {
                    waiting.resumed(connector);
                }
            }
            Err(error) if error.is_fatal() => {
                return Err(error).context("monitor loop stopped because click injection failed");
            }
            Err(error) => match waiting.as_mut() {
                Some(waiting) => waiting.attempt_failed(connector, error.to_string(), &backoff),
                // A disconnected pointer is waited out like a disconnected
                // capture: neither client can work again until rebuilt.
                None if error.disconnect().is_some() => {
                    let what = match error.disconnect() {
                        Some(Disconnect::Unresponsive) => "stopped answering",
                        _ => "disconnected",
                    };
                    warn!(cause = %error, "output {connector} {what}; waiting for it to return");
                    waiting = Some(Waiting::new(&error, &backoff));
                }
                None => {
                    consecutive_failures += 1;
                    let stage = error.stage_label();
                    if consecutive_failures >= MAX_CONSECUTIVE_CYCLE_FAILURES {
                        warn!(
                            stage,
                            cause = %error,
                            "output {connector} failed {consecutive_failures} consecutive \
                             cycles; waiting for it to recover"
                        );
                        consecutive_failures = 0;
                        waiting = Some(Waiting::new(&error, &backoff));
                    } else {
                        warn!(
                            stage,
                            consecutive_failures,
                            error = %error,
                            "cycle skipped after runtime failure"
                        );
                    }
                }
            },
        }

        if let Some(waiting) = waiting.as_mut() {
            match waiting.reconnect(connector, &backoff, &shutdown_rx, &mut cycles) {
                // Fresh clients capture at once: that cycle decides the wait.
                ReconnectOutcome::Reconnected => continue,
                ReconnectOutcome::Shutdown => break,
            }
        }

        // Wait out only what is left of the interval. Sleeping the full interval
        // after the cycle would make the real scan period `interval_ms` plus the
        // capture and match time, which drifts further apart the slower a cycle is.
        let remaining = interval.saturating_sub(cycle_started.elapsed());
        if shutdown_within(&shutdown_rx, remaining) {
            break;
        }
    }

    Ok(())
}

/// Why a template that may be on screen could not be clicked in one cycle.
enum Unclickable<'a> {
    /// Found in grayscale, but its best match failed the color check.
    ColorRejected {
        score: Option<f64>,
        comparison: &'a matcher::ColorComparison,
    },
    /// Larger than the captured frame, so it could not be scored at all.
    LargerThanFrame,
}

/// Counts, per template, the consecutive cycles it stayed unclickable, and
/// warns about a template that stays so for [`UNCLICKABLE_STREAK_CYCLES`].
///
/// Each case is logged at debug level every cycle, but a template that can
/// never be clicked, such as one captured from a dimmed button or at another
/// resolution, would otherwise fail silently at the default level forever.
pub(crate) struct UnclickableStreaks {
    streak_cycles: u32,
    warning_interval: Duration,
    streaks: BTreeMap<String, UnclickableStreak>,
}

struct UnclickableStreak {
    cycles: u32,
    last_warning: Option<Instant>,
}

impl Default for UnclickableStreaks {
    fn default() -> Self {
        Self::new(UNCLICKABLE_STREAK_CYCLES, UNCLICKABLE_WARNING_INTERVAL)
    }
}

impl UnclickableStreaks {
    /// Warns once a template stayed unclickable `streak_cycles` cycles in a
    /// row, then at most once per `warning_interval` while it stays so.
    fn new(streak_cycles: u32, warning_interval: Duration) -> Self {
        Self {
            streak_cycles,
            warning_interval,
            streaks: BTreeMap::new(),
        }
    }

    /// Records one cycle's scan of every template. A template accepted by the
    /// color check, or simply not found, ends its streak.
    fn observe(
        &mut self,
        prepared_rules: &[PreparedRule],
        scores: &TemplateScores,
        rejections: &ColorRejections,
        frame: (i32, i32),
    ) {
        for (template, score) in scores {
            let unclickable = match (rejections.get(template), score) {
                (Some(comparison), _) => Unclickable::ColorRejected {
                    score: *score,
                    comparison,
                },
                (None, None) => Unclickable::LargerThanFrame,
                (None, Some(_)) => {
                    self.streaks.remove(template);
                    continue;
                }
            };
            let streak = self
                .streaks
                .entry(template.clone())
                .or_insert(UnclickableStreak {
                    cycles: 0,
                    last_warning: None,
                });
            streak.cycles += 1;
            let warned_recently = streak
                .last_warning
                .is_some_and(|at| at.elapsed() < self.warning_interval);
            if streak.cycles < self.streak_cycles || warned_recently {
                continue;
            }
            streak.last_warning = Some(Instant::now());
            match unclickable {
                Unclickable::ColorRejected { score, comparison } => warn!(
                    target_template = %template,
                    cycles = streak.cycles,
                    score = score.unwrap_or(f64::NAN),
                    max_channel_delta = comparison.max_channel_delta(),
                    contrast_ratio = comparison.contrast_ratio,
                    "template {template} keeps matching in grayscale but failing the color check"
                ),
                Unclickable::LargerThanFrame => {
                    let size = prepared_rules
                        .iter()
                        .find(|rule| &rule.target_template == template)
                        .map(|rule| format!("{}x{}", rule.template_size.0, rule.template_size.1))
                        .unwrap_or_else(|| "unknown-size".to_string());
                    warn!(
                        target_template = %template,
                        cycles = streak.cycles,
                        template_size = %size,
                        frame_size = %format!("{}x{}", frame.0, frame.1),
                        "template {template} is larger than the captured frame and cannot be matched"
                    );
                }
            }
        }
    }
}

/// Executes one full runtime cycle: capture, match, verify colors, evaluate
/// rules, and click.
///
/// `history` gives the rule clicked last its turn order and records which
/// templates stayed unclickable; the cycle updates both for the next one.
pub(crate) fn run_cycle<S: FrameSource>(
    rules_config: &[RuleConfig],
    prepared_rules: &[PreparedRule],
    match_threshold: f32,
    monitor: &MonitorSpec,
    capture: &mut CaptureService<S>,
    executor: &mut impl ClickExecutor,
    history: &mut CycleHistory,
) -> std::result::Result<Option<PlannedClick>, RuntimeCycleError> {
    // Capturing needs the service mutably and the color check after matching
    // reads the frame it kept; the two closures run one after the other.
    let capture = RefCell::new(capture);
    let (previous_click, unclickable) = (history.previous_click, &mut history.unclickable);
    let outcome = run_cycle_with(
        rules_config,
        prepared_rules,
        match_threshold,
        monitor,
        || capture.borrow_mut().capture_monitor(),
        |screenshot, threshold| {
            let (mut matches, scores) =
                matcher::scan_all_scored(screenshot, prepared_rules, threshold).with_context(
                    || format!("OpenCV matching failed at threshold {:.2}", match_threshold),
                )?;
            let capture = capture.borrow();
            let rejections = matcher::verify_colors(&mut matches, prepared_rules, |region| {
                capture.region_stats(region)
            })?;
            let frame = (screenshot.cols(), screenshot.rows());
            unclickable.observe(prepared_rules, &scores, &rejections, frame);
            Ok(matches)
        },
        |matches, extent| {
            execute_match_set(rules_config, extent, matches, previous_click, executor)
        },
    );
    history.previous_click = rule_clicked_by(&outcome);
    outcome
}

fn run_cycle_with<C, M, E>(
    rules_config: &[RuleConfig],
    prepared_rules: &[PreparedRule],
    match_threshold: f32,
    monitor: &MonitorSpec,
    capture_screenshot: C,
    scan_matches: M,
    execute_cycle: E,
) -> std::result::Result<Option<PlannedClick>, RuntimeCycleError>
where
    C: FnOnce() -> Result<CapturedImage>,
    M: FnOnce(&Mat, f32) -> Result<MatchSet>,
    E: FnOnce(&MatchSet, ImageExtent) -> Result<Option<PlannedClick>>,
{
    let screenshot = capture_screenshot().map_err(RuntimeCycleError::Capture)?;
    debug!(
        monitor = %monitor.name,
        width = screenshot.extent.width,
        height = screenshot.extent.height,
        "captured screenshot"
    );
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

/// Executes at most one Wayland click for a rule whose template matched this
/// cycle's screenshot.
///
/// `previous_click` is the rule clicked in the previous cycle; matched rules
/// after it get the turn first (see [`rules::evaluate_rules`]). With `None`
/// the first matched rule in configuration order is clicked.
pub fn execute_match_set(
    rules_config: &[RuleConfig],
    extent: ImageExtent,
    matches: &MatchSet,
    previous_click: Option<usize>,
    executor: &mut impl ClickExecutor,
) -> Result<Option<PlannedClick>> {
    let planned = rules::evaluate_rules(rules_config, matches, extent, previous_click);
    if let Some(click) = &planned {
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
