fn capture_mat(width: i32, height: i32) -> Mat {
    Mat::new_rows_cols_with_default(height, width, CV_8UC1, Scalar::all(0.0)).unwrap()
}

fn prepared_rule(target_template: &str, template_path: &str) -> PreparedRule {
    PreparedRule {
        target_template: target_template.to_string(),
        template_path: std::path::PathBuf::from(template_path),
        template_size: (20, 10),
        template_mat: std::sync::Arc::new(
            Mat::new_rows_cols_with_default(10, 20, CV_8UC1, Scalar::all(255.0)).unwrap(),
        ),
        template_colors: crate::matcher::ColorStats { mean_bgr: [255.0; 3], luma_std: 0.0 },
    }
}

fn match_failure(message: &'static str) -> RuntimeCycleError {
    RuntimeCycleError::Match(anyhow!(message))
}

fn click_failure(message: &'static str) -> RuntimeCycleError {
    RuntimeCycleError::Click(anyhow!(message))
}

#[derive(Default)]
struct RecordingExecutor {
    clicks: Vec<crate::wayland_pointer::PlannedClick>,
    failure: Option<anyhow::Error>,
}

impl crate::wayland_pointer::ClickExecutor for RecordingExecutor {
    fn click(&mut self, click: &crate::wayland_pointer::PlannedClick) -> anyhow::Result<()> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        self.clicks.push(click.clone());
        Ok(())
    }
}

#[test]
fn plans_only_the_first_matching_rule_from_the_match_set() {
    let _monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 0,
        origin_y: 0,
    };
    let matches = MatchSet::from([
        (
            "accept_button.png".to_string(),
            vec![MatchRegion {
                left: 10,
                top: 20,
                width: 20,
                height: 10,
            }],
        ),
        (
            "ready_button.png".to_string(),
            vec![MatchRegion {
                left: 40,
                top: 50,
                width: 20,
                height: 10,
            }],
        ),
    ]);
    let rules = vec![
        crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        },
        crate::config::RuleConfig {
            target_template: "ready_button.png".to_string(),
        },
    ];

    let planned = crate::rules::evaluate_rules(
        &rules,
        &matches,
        crate::wayland_pointer::ImageExtent { width: 1920, height: 1080 },
        None,
    )
    .expect("both rules matched");

    assert_eq!(planned.rule_index, 0);
}

fn capture_failure(message: &'static str) -> RuntimeCycleError {
    RuntimeCycleError::Capture(anyhow!(message))
}

/// Runs the monitor loop for output DP-1 with millisecond recovery timing.
fn run_loop(
    interval_ms: u64,
    shutdown_rx: Receiver<()>,
    cycles: impl MonitorCycles,
) -> anyhow::Result<()> {
    run_monitor_loop_with_runner("DP-1", interval_ms, TEST_BACKOFF, shutdown_rx, cycles)
}

#[test]
fn loop_stops_promptly_when_shutdown_arrives_during_the_interval_wait() {
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let (cycle_tx, cycle_rx) = std::sync::mpsc::channel();
    let requester = std::thread::spawn(move || {
        cycle_rx.recv().unwrap();
        shutdown_tx.send(()).unwrap();
    });
    let mut calls = 0_usize;

    let started = std::time::Instant::now();
    run_loop(60_000, shutdown_rx, || {
        calls += 1;
        cycle_tx.send(()).unwrap();
        Ok(())
    })
    .unwrap();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "shutdown took {:?}",
        started.elapsed()
    );
    assert_eq!(calls, 1);
    requester.join().unwrap();
}

#[test]
fn too_many_consecutive_non_click_failures_wait_for_the_output_instead_of_exiting() {
    let (shutdown_tx, rx) = std::sync::mpsc::channel();
    let mut calls = 0_usize;

    let logs = crate::support::capture_debug_logs(|| {
        run_loop(1, rx, || {
            calls += 1;
            match calls {
                calls if calls < MAX_CONSECUTIVE_CYCLE_FAILURES => {
                    Err(match_failure("temporary matcher failure"))
                }
                calls if calls == MAX_CONSECUTIVE_CYCLE_FAILURES => {
                    Err(capture_failure("screencopy frame did not complete"))
                }
                _ => {
                    shutdown_tx.send(()).unwrap();
                    Ok(())
                }
            }
        })
        .unwrap();
    });

    assert_eq!(calls, MAX_CONSECUTIVE_CYCLE_FAILURES + 1);
    assert_eq!(logs.matches("cycle skipped").count(), MAX_CONSECUTIVE_CYCLE_FAILURES - 1);
    assert_eq!(
        logs.matches(&format!(
            "output DP-1 failed {MAX_CONSECUTIVE_CYCLE_FAILURES} consecutive cycles; \
             waiting for it to recover"
        ))
        .count(),
        1,
        "{logs}"
    );
    assert!(
        logs.contains("cause=capture failed: screencopy frame did not complete"),
        "{logs}"
    );
    assert_eq!(logs.matches("monitoring of output DP-1 resumed").count(), 1, "{logs}");
    // The recovery is logged at the same level as the warning that opened the
    // wait, so the default `warn` filter shows when the outage ended.
    assert!(
        logs.lines()
            .any(|line| line.contains("WARN") && line.contains("output DP-1 resumed")),
        "{logs}"
    );
}

#[test]
fn a_successful_cycle_resets_the_consecutive_failure_count() {
    let (shutdown_tx, rx) = std::sync::mpsc::channel();
    let mut calls = 0_usize;

    let logs = crate::support::capture_debug_logs(|| {
        run_loop(1, rx, || {
            calls += 1;
            // One short of the limit, a success, then failures until the
            // limit is reached again, counted from the success.
            if calls == 2 * MAX_CONSECUTIVE_CYCLE_FAILURES {
                shutdown_tx.send(()).unwrap();
            }
            if calls == MAX_CONSECUTIVE_CYCLE_FAILURES {
                return Ok(());
            }
            Err(match_failure("temporary matcher failure"))
        })
        .unwrap();
    });

    assert_eq!(calls, 2 * MAX_CONSECUTIVE_CYCLE_FAILURES);
    assert_eq!(logs.matches("cycle skipped").count(), 2 * (MAX_CONSECUTIVE_CYCLE_FAILURES - 1));
    assert_eq!(logs.matches("consecutive cycles; waiting for it to recover").count(), 1);
}

#[test]
fn click_failure_stops_immediately_even_after_earlier_skipped_cycles() {
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut calls = 0_usize;

    let error = run_loop(1, rx, || {
        calls += 1;
        if calls == 2 {
            return Err(click_failure("Wayland virtual pointer disconnected"));
        }
        Err(match_failure("temporary matcher failure"))
    })
    .unwrap_err();

    assert_eq!(calls, 2);
    assert!(error
        .to_string()
        .contains("monitor loop stopped because click injection failed"));
}

#[test]
fn continues_monitoring_after_transient_cycle_failure() {
    let (tx, rx) = std::sync::mpsc::channel();
    let calls = Arc::new(Mutex::new(0_usize));
    let calls_for_runner = Arc::clone(&calls);

    run_loop(1, rx, move || {
        let mut value = calls_for_runner.lock().unwrap();
        *value += 1;
        if *value == 2 {
            tx.send(()).unwrap();
            return Ok(());
        }

        Err(match_failure("temporary matcher failure"))
    })
    .unwrap();

    assert_eq!(*calls.lock().unwrap(), 2);
}

#[test]
fn stops_monitoring_after_click_injection_failure() {
    let (_tx, rx) = std::sync::mpsc::channel();
    let calls = Arc::new(Mutex::new(0_usize));
    let calls_for_runner = Arc::clone(&calls);

    let error = run_loop(1, rx, move || {
        *calls_for_runner.lock().unwrap() += 1;
        Err(click_failure("Wayland virtual pointer disconnected"))
    })
    .unwrap_err();

    assert_eq!(*calls.lock().unwrap(), 1);
    assert!(error
        .to_string()
        .contains("monitor loop stopped because click injection failed"));
    assert!(format!("{error:#}").contains("Wayland virtual pointer disconnected"));
}

#[test]
fn classifies_capture_failures_by_stage() {
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 0,
        origin_y: 0,
    };

    let error = run_cycle_with(
        &[],
        &[],
        0.95,
        &monitor,
        || Err(anyhow!("screencopy frame failed")),
        |_, _| Ok(MatchSet::new()),
        |_, _| Ok(None),
    )
    .unwrap_err();

    assert_eq!(error.stage_label(), "capture");
    assert!(error.to_string().contains("screencopy frame failed"));
}

#[test]
fn classifies_match_failures_by_stage() {
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 0,
        origin_y: 0,
    };

    let error = run_cycle_with(
        &[],
        &[],
        0.95,
        &monitor,
        || Ok(CapturedImage::from_decoded(capture_mat(1920, 1080)).unwrap()),
        |_, _| Err(anyhow!("OpenCV blew up")),
        |_, _| Ok(None),
    )
    .unwrap_err();

    assert_eq!(error.stage_label(), "OpenCV match");
    assert!(error.to_string().contains("OpenCV blew up"));
}

#[test]
fn classifies_click_failures_by_stage() {
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 0,
        origin_y: 0,
    };

    let error = run_cycle_with(
        &[],
        &[],
        0.95,
        &monitor,
        || Ok(CapturedImage::from_decoded(capture_mat(1920, 1080)).unwrap()),
        |_, _| Ok(MatchSet::new()),
        |_, _| Err(anyhow!("Wayland virtual pointer unavailable")),
    )
    .unwrap_err();

    assert_eq!(error.stage_label(), "click execution");
    assert!(error
        .to_string()
        .contains("Wayland virtual pointer unavailable"));
}

#[test]
fn continues_into_later_cycles_after_successful_clicks() {
    let (tx, rx) = std::sync::mpsc::channel();
    let calls = Arc::new(Mutex::new(0_usize));
    let calls_for_runner = Arc::clone(&calls);

    run_loop(1, rx, move || {
        let mut value = calls_for_runner.lock().unwrap();
        *value += 1;
        if *value == 3 {
            tx.send(()).unwrap();
        }
        Ok(())
    })
    .unwrap();

    assert_eq!(*calls.lock().unwrap(), 3);
}

#[test]
fn run_cycle_scans_all_rules_once_and_clicks_only_the_first_match() {
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 100,
        origin_y: 200,
    };
    let rules = vec![
        crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        },
        crate::config::RuleConfig {
            target_template: "ready_button.png".to_string(),
        },
    ];
    let prepared_rules = vec![
        prepared_rule("accept_button.png", "accept_button.png"),
        prepared_rule("ready_button.png", "ready_button.png"),
    ];
    let capture_calls = Arc::new(Mutex::new(0_usize));
    let capture_calls_for_closure = Arc::clone(&capture_calls);
    let match_calls = Arc::new(Mutex::new(0_usize));
    let match_calls_for_closure = Arc::clone(&match_calls);
    let rules_for_execution = rules.clone();
    let mut executor = RecordingExecutor::default();

    let planned = run_cycle_with(
        &rules,
        &prepared_rules,
        0.95,
        &monitor,
        move || {
            *capture_calls_for_closure.lock().unwrap() += 1;
            Ok(CapturedImage::from_decoded(capture_mat(1920, 1080)).unwrap())
        },
        move |_, threshold| {
            *match_calls_for_closure.lock().unwrap() += 1;
            assert_eq!(threshold, 0.95);
            Ok(MatchSet::from([
                (
                    "accept_button.png".to_string(),
                    vec![MatchRegion {
                        left: 10,
                        top: 20,
                        width: 20,
                        height: 10,
                    }],
                ),
                (
                    "ready_button.png".to_string(),
                    vec![MatchRegion {
                        left: 30,
                        top: 40,
                        width: 20,
                        height: 10,
                    }],
                ),
            ]))
        },
        |matches, extent| {
            execute_match_set(&rules_for_execution, extent, matches, None, &mut executor)
        },
    )
    .unwrap();

    let expected = crate::wayland_pointer::PlannedClick {
        rule_index: 0,
        target_template: "accept_button.png".to_string(),
        output_x: 20,
        output_y: 25,
        extent: crate::wayland_pointer::ImageExtent { width: 1920, height: 1080 },
    };
    assert_eq!(planned, Some(expected.clone()));
    assert_eq!(executor.clicks, vec![expected]);
    assert_eq!(*capture_calls.lock().unwrap(), 1);
    assert_eq!(*match_calls.lock().unwrap(), 1);
}

#[test]
fn execute_match_set_invokes_wayland_executor_with_output_local_plan() {
    let rules = vec![crate::config::RuleConfig { target_template: "accept_button.png".to_string() }];
    let matches = MatchSet::from([("accept_button.png".to_string(), vec![MatchRegion { left: 10, top: 20, width: 20, height: 10 }])]);
    let mut executor = RecordingExecutor::default();

    let planned = execute_match_set(
        &rules,
        crate::wayland_pointer::ImageExtent { width: 1920, height: 1080 },
        &matches,
        None,
        &mut executor,
    )
    .unwrap();

    assert_eq!(executor.clicks, planned.into_iter().collect::<Vec<_>>());
    assert_eq!(executor.clicks[0].output_x, 20);
    assert_eq!(executor.clicks[0].output_y, 25);
    assert_eq!(executor.clicks[0].extent, crate::wayland_pointer::ImageExtent { width: 1920, height: 1080 });
    assert_ne!((executor.clicks[0].output_x, executor.clicks[0].output_y), (120, 225));
}

#[test]
fn execute_match_set_surfaces_executor_failure() {
    let rules = vec![crate::config::RuleConfig { target_template: "accept_button.png".to_string() }];
    let matches = MatchSet::from([("accept_button.png".to_string(), vec![MatchRegion { left: 0, top: 0, width: 2, height: 2 }])]);
    let mut executor = RecordingExecutor { clicks: Vec::new(), failure: Some(anyhow!("selected output was removed")) };

    let error = execute_match_set(
        &rules,
        crate::wayland_pointer::ImageExtent { width: 2, height: 2 },
        &matches,
        None,
        &mut executor,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("selected output was removed"));
}

#[test]
fn hands_the_decoded_screenshot_to_the_matcher_untouched() {
    use opencv::prelude::{MatTrait, MatTraitConst};

    // The capture decodes the screenshot once and the matcher reuses that exact
    // matrix. Nothing else proves the pixels survive the handoff: every other
    // test feeds the matcher through a closure that ignores the image.
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 0,
        origin_y: 0,
    };
    let mut screenshot = capture_mat(6, 4);
    *screenshot.at_2d_mut::<u8>(3, 5).unwrap() = 231;

    let seen = Arc::new(Mutex::new(None));
    let seen_for_closure = Arc::clone(&seen);
    let mut executor = RecordingExecutor::default();

    run_cycle_with(
        &[],
        &[],
        0.95,
        &monitor,
        move || Ok(CapturedImage::from_decoded(screenshot).unwrap()),
        move |image, _| {
            *seen_for_closure.lock().unwrap() = Some((
                image.cols(),
                image.rows(),
                *image.at_2d::<u8>(3, 5).unwrap(),
                *image.at_2d::<u8>(0, 0).unwrap(),
            ));
            Ok(MatchSet::new())
        },
        |matches, extent| execute_match_set(&[], extent, matches, None, &mut executor),
    )
    .unwrap();

    assert_eq!(*seen.lock().unwrap(), Some((6, 4, 231, 0)));
}

#[test]
fn execute_match_set_sends_one_click_when_two_rules_match() {
    let rules = vec![
        crate::config::RuleConfig { target_template: "accept_button.png".to_string() },
        crate::config::RuleConfig { target_template: "ready_button.png".to_string() },
    ];
    let matches = MatchSet::from([
        (
            "accept_button.png".to_string(),
            vec![MatchRegion { left: 10, top: 20, width: 20, height: 10 }],
        ),
        (
            "ready_button.png".to_string(),
            vec![MatchRegion { left: 30, top: 40, width: 20, height: 10 }],
        ),
    ]);
    let mut executor = RecordingExecutor::default();

    execute_match_set(
        &rules,
        crate::wayland_pointer::ImageExtent { width: 1920, height: 1080 },
        &matches,
        None,
        &mut executor,
    )
    .unwrap();

    assert_eq!(executor.clicks.len(), 1);
    assert_eq!(executor.clicks[0].rule_index, 0);
    assert_eq!((executor.clicks[0].output_x, executor.clicks[0].output_y), (20, 25));
}

#[test]
fn consecutive_cycles_take_turns_and_reset_when_nothing_is_clicked() {
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 0,
        origin_y: 0,
    };
    let rules = vec![
        crate::config::RuleConfig { target_template: "accept_button.png".to_string() },
        crate::config::RuleConfig { target_template: "ready_button.png".to_string() },
    ];
    let both = || {
        MatchSet::from([
            (
                "accept_button.png".to_string(),
                vec![MatchRegion { left: 10, top: 20, width: 20, height: 10 }],
            ),
            (
                "ready_button.png".to_string(),
                vec![MatchRegion { left: 30, top: 40, width: 20, height: 10 }],
            ),
        ])
    };
    // Some(set) is a cycle that scans `set`; None is a cycle whose capture fails.
    let cycles = vec![
        Some(both()),
        Some(both()),
        Some(both()),
        Some(both()),
        Some(MatchSet::new()),
        Some(both()),
        None,
        Some(both()),
    ];
    let mut executor = RecordingExecutor::default();
    let mut previous_click = None;

    for cycle in cycles {
        let outcome = run_cycle_with(
            &rules,
            &[],
            0.95,
            &monitor,
            || match cycle {
                Some(_) => Ok(CapturedImage::from_decoded(capture_mat(1920, 1080)).unwrap()),
                None => Err(anyhow!("screencopy frame failed")),
            },
            |_, _| Ok(cycle.clone().unwrap_or_default()),
            |matches, extent| {
                execute_match_set(&rules, extent, matches, previous_click, &mut executor)
            },
        );
        previous_click = rule_clicked_by(&outcome);
    }

    let clicked: Vec<usize> = executor.clicks.iter().map(|click| click.rule_index).collect();
    // Two rules alternate; the empty cycle and the failed capture each restart
    // the rotation, so the click after them goes to the first rule again.
    assert_eq!(clicked, vec![0, 1, 0, 1, 0, 0]);
}

/// Hands out queued grayscale frames to the real capture service, sampling
/// regions of the last one as gray pixels.
struct ScriptedFrames {
    queued: std::collections::VecDeque<Mat>,
    last: Option<Mat>,
}

impl FrameSource for ScriptedFrames {
    fn capture_frame(&mut self) -> anyhow::Result<Mat> {
        let frame = self.queued.pop_front().ok_or_else(|| anyhow!("no frame queued"))?;
        self.last = Some(frame.clone());
        Ok(frame)
    }

    fn region_stats(&self, region: Rect) -> anyhow::Result<ColorStats> {
        sample_region(self.last.as_ref(), region)
    }
}

/// Serves one BGR screen, as grayscale to the matcher and in color to the check.
struct ColorScreen(Mat);

impl FrameSource for ColorScreen {
    fn capture_frame(&mut self) -> anyhow::Result<Mat> {
        Ok(crate::support::to_gray(&self.0))
    }

    fn region_stats(&self, region: Rect) -> anyhow::Result<ColorStats> {
        sample_region(Some(&self.0), region)
    }
}

fn sample_region(frame: Option<&Mat>, region: Rect) -> anyhow::Result<ColorStats> {
    use opencv::prelude::MatTraitConst;

    let frame = frame.ok_or_else(|| anyhow!("no captured frame"))?;
    crate::matcher::mat_color_stats(&frame.roi(region)?)
}

/// Records clicked rules and requests shutdown once `stop_after` clicks landed.
/// Clones share one log, so a reconnected executor continues it.
#[derive(Clone)]
struct ShutdownAfterClicks {
    clicked: std::rc::Rc<RefCell<Vec<usize>>>,
    stop_after: usize,
    shutdown_tx: std::sync::mpsc::Sender<()>,
    _dropped: DropCounter,
}

impl crate::wayland_pointer::ClickExecutor for ShutdownAfterClicks {
    fn click(&mut self, click: &crate::wayland_pointer::PlannedClick) -> anyhow::Result<()> {
        let mut clicked = self.clicked.borrow_mut();
        clicked.push(click.rule_index);
        if clicked.len() == self.stop_after {
            self.shutdown_tx.send(()).unwrap();
        }
        Ok(())
    }
}

/// Counts how many of its holders were dropped.
#[derive(Clone, Default)]
struct DropCounter(std::rc::Rc<std::cell::Cell<usize>>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

/// Serves its frames, then fails every capture as a removed DP-1 output.
struct FramesUntilRemoved {
    frames: std::collections::VecDeque<Mat>,
    last: Option<Mat>,
    _dropped: DropCounter,
}

impl FrameSource for FramesUntilRemoved {
    fn capture_frame(&mut self) -> anyhow::Result<Mat> {
        let frame = self.frames.pop_front().ok_or_else(removed_output)?;
        self.last = Some(frame.clone());
        Ok(frame)
    }

    fn region_stats(&self, region: Rect) -> anyhow::Result<ColorStats> {
        sample_region(self.last.as_ref(), region)
    }
}

fn removed_output() -> anyhow::Error {
    anyhow::Error::new(Disconnect::OutputRemoved { connector: "DP-1".into() })
}

/// Reconnect timing in milliseconds instead of seconds.
const TEST_BACKOFF: ReconnectBackoff = ReconnectBackoff {
    first_delay: Duration::from_millis(5),
    max_delay: Duration::from_millis(20),
    reminder_interval: Duration::from_secs(3600),
};

/// Two rules whose distinct high-contrast patterns both appear in the
/// returned frame, with the config and monitor to scan it.
fn two_rule_setup() -> (Mat, Mat, Vec<PreparedRule>, crate::config::AppConfig, MonitorSpec) {
    // Two high-contrast patterns that do not resemble each other: a vertical
    // and a horizontal black/white split.
    let vertical = pattern_mat(|_, col| if col < 6 { 255 } else { 0 });
    let horizontal = pattern_mat(|row, _| if row < 6 { 255 } else { 0 });
    let both = frame_with(&[(&vertical, 4, 4), (&horizontal, 40, 12)]);
    let rule = |name: &str, template: &Mat| PreparedRule {
        target_template: name.to_string(),
        template_path: std::path::PathBuf::from(name),
        template_size: (12, 12),
        template_mat: Arc::new(template.clone()),
        template_colors: crate::matcher::mat_color_stats(template).unwrap(),
    };
    let prepared_rules = vec![rule("vertical.png", &vertical), rule("horizontal.png", &horizontal)];
    let config = crate::config::AppConfig {
        monitor_name: "DP-1".to_string(),
        interval_ms: 1,
        match_threshold: 0.9,
        rules: vec![
            crate::config::RuleConfig { target_template: "vertical.png".to_string() },
            crate::config::RuleConfig { target_template: "horizontal.png".to_string() },
        ],
    };
    let monitor = MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 64,
        height: 32,
        origin_x: 0,
        origin_y: 0,
    };
    (both, frame_with(&[]), prepared_rules, config, monitor)
}

/// A 12x12 grayscale matrix whose pixel at (`row`, `col`) is `pixel(row, col)`.
fn pattern_mat(pixel: impl Fn(i32, i32) -> u8) -> Mat {
    use opencv::prelude::MatTrait;

    let mut mat = Mat::new_rows_cols_with_default(12, 12, CV_8UC1, Scalar::all(0.0)).unwrap();
    for row in 0..12 {
        for col in 0..12 {
            *mat.at_2d_mut::<u8>(row, col).unwrap() = pixel(row, col);
        }
    }
    mat
}

/// A uniform mid-gray 64x32 frame with each pattern stamped at its (left, top).
fn frame_with(patterns: &[(&Mat, i32, i32)]) -> Mat {
    use opencv::prelude::{MatTrait, MatTraitConst};

    let mut frame = Mat::new_rows_cols_with_default(32, 64, CV_8UC1, Scalar::all(128.0)).unwrap();
    for (pattern, left, top) in patterns {
        for row in 0..pattern.rows() {
            for col in 0..pattern.cols() {
                *frame.at_2d_mut::<u8>(top + row, left + col).unwrap() =
                    *pattern.at_2d::<u8>(row, col).unwrap();
            }
        }
    }
    frame
}

#[test]
fn the_monitor_loop_alternates_matched_rules_and_resets_after_an_empty_frame() {
    let (both, neither, prepared_rules, config, monitor) = two_rule_setup();
    let frames = [&both, &both, &both, &neither, &both, &both];
    let source = ScriptedFrames {
        queued: frames.iter().map(|frame| (*frame).clone()).collect(),
        last: None,
    };
    let capture = CaptureService::with_source("DP-1", source);
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let executor = ShutdownAfterClicks {
        clicked: Default::default(),
        stop_after: 5,
        shutdown_tx,
        _dropped: DropCounter::default(),
    };
    let clicked = executor.clicked.clone();

    run_monitor_loop(
        &config,
        &prepared_rules,
        &monitor,
        (capture, executor),
        |_: &str| Err(anyhow!("no reconnect expected")),
        shutdown_rx,
    )
    .unwrap();

    // Both rules match every `both` frame, so they take turns; the frame with
    // no match clicks nothing and restarts the turn at the first rule, even
    // though the first rule was the one clicked just before it.
    assert_eq!(*clicked.borrow(), vec![0, 1, 0, 0, 1]);
}

#[test]
fn a_removed_output_is_waited_out_and_both_clients_reconnect_with_fresh_turns() {
    let (both, _, prepared_rules, config, monitor) = two_rule_setup();
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let old_clients = DropCounter::default();
    let executor = ShutdownAfterClicks {
        clicked: Default::default(),
        stop_after: 3,
        shutdown_tx,
        _dropped: old_clients.clone(),
    };
    let clicked = executor.clicked.clone();
    let first_source = FramesUntilRemoved {
        frames: [both.clone()].into(),
        last: None,
        _dropped: old_clients.clone(),
    };
    let reconnected_executor = ShutdownAfterClicks {
        clicked: clicked.clone(),
        stop_after: 3,
        shutdown_tx: executor.shutdown_tx.clone(),
        _dropped: DropCounter::default(),
    };
    let mut attempts = Vec::new();

    let logs = crate::support::capture_debug_logs(|| {
        run_monitor_loop_with_backoff(
            &config,
            &prepared_rules,
            &monitor,
            (CaptureService::with_source("DP-1", first_source), executor),
            |connector: &str| {
                // Both old clients are gone before any reconnect attempt: the
                // probe's own clone plus the source's and the executor's.
                attempts.push((connector.to_string(), old_clients.0.get()));
                if attempts.len() == 1 {
                    return Err(anyhow!("configured connector DP-1 was not found"));
                }
                let source = FramesUntilRemoved {
                    frames: [both.clone(), both.clone()].into(),
                    last: None,
                    _dropped: DropCounter::default(),
                };
                Ok((CaptureService::with_source(connector, source), reconnected_executor.clone()))
            },
            TEST_BACKOFF,
            shutdown_rx,
        )
        .unwrap();
    });

    assert_eq!(attempts, vec![("DP-1".to_string(), 2), ("DP-1".to_string(), 2)]);
    // Rule 0 was clicked last before the disconnect, yet the reconnected
    // output starts its turns at rule 0 again.
    assert_eq!(*clicked.borrow(), vec![0, 0, 1]);
    assert_eq!(logs.matches("output DP-1 disconnected; waiting for it to return").count(), 1);
    assert_eq!(logs.matches("monitoring of output DP-1 resumed").count(), 1, "{logs}");
    assert!(!logs.contains("cycle skipped"), "a disconnect is not a skipped cycle:\n{logs}");
}

#[test]
fn shutdown_during_the_reconnect_wait_stops_the_loop_promptly() {
    let (_, _, prepared_rules, config, monitor) = two_rule_setup();
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let executor = ShutdownAfterClicks {
        clicked: Default::default(),
        stop_after: usize::MAX,
        shutdown_tx: shutdown_tx.clone(),
        _dropped: DropCounter::default(),
    };
    let source = FramesUntilRemoved {
        frames: Default::default(),
        last: None,
        _dropped: DropCounter::default(),
    };
    let requester = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        shutdown_tx.send(()).unwrap();
    });
    let mut attempts = 0;

    let started = Instant::now();
    run_monitor_loop_with_backoff(
        &config,
        &prepared_rules,
        &monitor,
        (CaptureService::with_source("DP-1", source), executor),
        |_: &str| {
            attempts += 1;
            Err(anyhow!("configured connector DP-1 was not found"))
        },
        ReconnectBackoff { first_delay: Duration::from_secs(60), ..TEST_BACKOFF },
        shutdown_rx,
    )
    .unwrap();

    assert!(started.elapsed() < Duration::from_secs(5), "stopped after {:?}", started.elapsed());
    assert_eq!(attempts, 0);
    requester.join().unwrap();
}

#[test]
fn the_standard_backoff_doubles_from_one_second_to_a_ten_second_cap() {
    let backoff = ReconnectBackoff::STANDARD;
    let delays: Vec<u64> = std::iter::successors(Some(backoff.first_delay), |delay| {
        Some(backoff.next_delay(*delay))
    })
    .take(7)
    .map(|delay| delay.as_secs())
    .collect();

    assert_eq!(delays, vec![1, 2, 4, 8, 10, 10, 10]);
    assert_eq!(backoff.reminder_interval, Duration::from_secs(60));
}

/// One scripted cycle result for [`ScriptedCycles`].
#[derive(Clone, Copy)]
enum Step {
    Succeeds,
    MatchFails,
    /// A frame the compositor never finished, as from a powered-off output.
    CaptureTimesOut,
    CaptureDisconnects,
    ClickDisconnects,
    /// A click whose round trip got no answer within its deadline.
    ClickStalls,
}

/// What a [`ScriptedCycles`] run did, shared with the test after the loop.
#[derive(Default)]
struct ScriptLog {
    cycles: Vec<Instant>,
    reconnects: Vec<Instant>,
}

/// Plays back cycle results, then requests shutdown with one last successful
/// cycle once the script runs out. Reconnects fail as long as `failed_reconnects`
/// lasts, then succeed.
struct ScriptedCycles {
    steps: std::collections::VecDeque<Step>,
    failed_reconnects: usize,
    shutdown_tx: std::sync::mpsc::Sender<()>,
    log: std::rc::Rc<RefCell<ScriptLog>>,
}

impl ScriptedCycles {
    fn new(
        steps: impl IntoIterator<Item = Step>,
        shutdown_tx: std::sync::mpsc::Sender<()>,
    ) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            failed_reconnects: 0,
            shutdown_tx,
            log: Default::default(),
        }
    }
}

impl MonitorCycles for ScriptedCycles {
    fn run_cycle(&mut self) -> std::result::Result<(), RuntimeCycleError> {
        self.log.borrow_mut().cycles.push(Instant::now());
        let Some(step) = self.steps.pop_front() else {
            self.shutdown_tx.send(()).unwrap();
            return Ok(());
        };
        match step {
            Step::Succeeds => Ok(()),
            Step::MatchFails => Err(match_failure("temporary matcher failure")),
            Step::CaptureTimesOut => Err(capture_failure(
                "screencopy frame for output DP-1 did not complete within 2000 ms",
            )),
            Step::CaptureDisconnects => Err(RuntimeCycleError::Capture(removed_output())),
            Step::ClickDisconnects => Err(RuntimeCycleError::Click(
                removed_output().context("Wayland virtual-pointer click failed"),
            )),
            Step::ClickStalls => Err(RuntimeCycleError::Click(
                anyhow::Error::new(Disconnect::Unresponsive)
                    .context("virtual-pointer barrier roundtrip failed within its 2000 ms deadline")
                    .context("Wayland virtual-pointer click failed"),
            )),
        }
    }

    fn reconnect(&mut self) -> anyhow::Result<()> {
        self.log.borrow_mut().reconnects.push(Instant::now());
        if self.failed_reconnects > 0 {
            self.failed_reconnects -= 1;
            return Err(anyhow!("configured connector DP-1 was not found"));
        }
        Ok(())
    }
}

#[test]
fn five_capture_timeouts_wait_for_the_output_and_the_first_success_resumes_monitoring() {
    use Step::*;
    let (shutdown_tx, rx) = std::sync::mpsc::channel();
    let timeouts = [CaptureTimesOut; MAX_CONSECUTIVE_CYCLE_FAILURES];
    let cycles = ScriptedCycles::new(
        timeouts.into_iter().chain([CaptureTimesOut, Succeeds, Succeeds]),
        shutdown_tx,
    );
    let log = cycles.log.clone();

    let mut result = None;
    let logs = crate::support::capture_debug_logs(|| result = Some(run_loop(1, rx, cycles)));

    // A powered-off output keeps timing out, even right after a reconnect;
    // the loop keeps waiting for it rather than exiting.
    result.unwrap().unwrap();
    assert_eq!(logs.matches("cycle skipped").count(), MAX_CONSECUTIVE_CYCLE_FAILURES - 1);
    assert_eq!(logs.matches("consecutive cycles; waiting for it to recover").count(), 1);
    assert_eq!(logs.matches("recovery attempt failed").count(), 1, "{logs}");
    assert_eq!(logs.matches("monitoring of output DP-1 resumed").count(), 1, "{logs}");
    // Two attempts: one whose cycle timed out, one whose cycle succeeded;
    // after that, normal cycles: one more success and the final one.
    let log = log.borrow();
    assert_eq!(log.reconnects.len(), 2);
    assert_eq!(log.cycles.len(), MAX_CONSECUTIVE_CYCLE_FAILURES + 4);
}

#[test]
fn shutdown_while_waiting_after_repeated_failures_stops_the_loop_promptly() {
    let (shutdown_tx, rx) = std::sync::mpsc::channel();
    let cycles = ScriptedCycles::new(
        [Step::CaptureTimesOut; MAX_CONSECUTIVE_CYCLE_FAILURES],
        shutdown_tx.clone(),
    );
    let log = cycles.log.clone();
    let requester = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        shutdown_tx.send(()).unwrap();
    });

    let started = Instant::now();
    run_monitor_loop_with_runner(
        "DP-1",
        1,
        ReconnectBackoff { first_delay: Duration::from_secs(60), ..TEST_BACKOFF },
        rx,
        cycles,
    )
    .unwrap();

    assert!(started.elapsed() < Duration::from_secs(5), "stopped after {:?}", started.elapsed());
    assert_eq!(log.borrow().reconnects.len(), 0);
    requester.join().unwrap();
}

#[test]
fn a_disconnect_waits_at_once_and_no_failure_while_waiting_ends_the_loop() {
    use Step::*;
    let (shutdown_tx, rx) = std::sync::mpsc::channel();
    let four_failures = [MatchFails; MAX_CONSECUTIVE_CYCLE_FAILURES - 1];
    let steps = four_failures
        .into_iter()
        .chain([CaptureDisconnects])
        // While waiting, failures of any kind but a fatal click keep waiting.
        .chain([MatchFails; 2 * MAX_CONSECUTIVE_CYCLE_FAILURES])
        .chain([CaptureTimesOut, ClickDisconnects, ClickStalls, Succeeds])
        .chain(four_failures)
        .chain([ClickDisconnects, Succeeds]);
    let cycles = ScriptedCycles::new(steps, shutdown_tx);
    let log = cycles.log.clone();

    let mut result = None;
    let logs = crate::support::capture_debug_logs(|| result = Some(run_loop(1, rx, cycles)));

    result.unwrap().unwrap();
    // Each disconnect follows four failures, yet neither run reaches the limit.
    assert_eq!(logs.matches("consecutive cycles; waiting").count(), 0, "{logs}");
    assert_eq!(logs.matches("output DP-1 disconnected; waiting for it to return").count(), 2);
    assert_eq!(logs.matches("monitoring of output DP-1 resumed").count(), 2);
    let while_waiting = 2 * MAX_CONSECUTIVE_CYCLE_FAILURES + 3;
    assert_eq!(log.borrow().reconnects.len(), while_waiting + 1 + 1);
}

#[test]
fn a_stalled_click_waits_for_the_output_instead_of_stopping_the_loop() {
    use Step::*;
    let (shutdown_tx, rx) = std::sync::mpsc::channel();
    let cycles = ScriptedCycles::new([ClickStalls, Succeeds], shutdown_tx);
    let log = cycles.log.clone();

    let mut result = None;
    let logs = crate::support::capture_debug_logs(|| result = Some(run_loop(1, rx, cycles)));

    result.unwrap().unwrap();
    assert_eq!(
        logs.matches("output DP-1 stopped answering; waiting for it to return").count(),
        1,
        "{logs}"
    );
    assert!(logs.contains("the Wayland compositor stopped answering"), "{logs}");
    assert_eq!(logs.matches("monitoring of output DP-1 resumed").count(), 1);
    assert_eq!(log.borrow().reconnects.len(), 1);
}

#[test]
fn waiting_backs_off_until_a_cycle_succeeds_and_reminds_with_the_last_error() {
    use Step::*;
    let backoff = ReconnectBackoff {
        first_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(160),
        reminder_interval: Duration::ZERO,
    };
    let (shutdown_tx, rx) = std::sync::mpsc::channel();
    // Two reconnects fail; then the cycles on the next two fresh clients
    // fail before the third succeeds. A later disconnect starts over.
    let mut cycles = ScriptedCycles::new(
        [CaptureDisconnects, MatchFails, MatchFails, Succeeds, CaptureDisconnects, Succeeds],
        shutdown_tx,
    );
    cycles.failed_reconnects = 2;
    let log = cycles.log.clone();

    let mut result = None;
    let logs = crate::support::capture_debug_logs(|| {
        result = Some(run_monitor_loop_with_runner("DP-1", 1, backoff, rx, cycles));
    });

    result.unwrap().unwrap();
    let log = log.borrow();
    let [first_disconnect, .., second_disconnect, _, _] = log.cycles[..] else {
        panic!("unexpected cycles: {}", log.cycles.len());
    };
    let reconnects = &log.reconnects;
    assert_eq!(reconnects.len(), 6);
    // Neither a failed reconnect nor a successful one whose cycle failed
    // resets the delay: it keeps doubling up to the cap until a cycle succeeds.
    let expected = [10, 20, 40, 80, 160].map(Duration::from_millis);
    let mut previous = first_disconnect;
    for (attempt, (&at, delay)) in reconnects.iter().zip(expected).enumerate() {
        let waited = at - previous;
        assert!(waited >= delay, "attempt {} came after {waited:?}, not {delay:?}", attempt + 1);
        previous = at;
    }
    // A success reset it: the next disconnect waits the first delay again.
    let waited = reconnects[5] - second_disconnect;
    assert!(
        waited >= Duration::from_millis(10) && waited < Duration::from_millis(160),
        "the attempt after a success came after {waited:?}"
    );
    assert_eq!(logs.matches("recovery attempt failed").count(), 4, "{logs}");
    assert_eq!(logs.matches("output DP-1 is still unavailable").count(), 4, "{logs}");
    assert_eq!(
        logs.matches("last_error=configured connector DP-1 was not found").count(),
        2,
        "{logs}"
    );
    assert_eq!(
        logs.matches("last_error=OpenCV match failed: temporary matcher failure").count(),
        2,
        "{logs}"
    );
}

#[test]
fn a_gray_match_that_fails_the_color_check_plans_no_click_and_the_cycle_succeeds() {
    use crate::support::{blend_toward, button, map_pixels, screen_with, to_gray};
    use crate::support::{BUTTON_FILL, BUTTON_TEXT, SCREEN_BACKGROUND};

    let exact = button(BUTTON_FILL, BUTTON_TEXT);
    let dimmed = map_pixels(&exact, |pixel| blend_toward(pixel, 128, 0.4));
    let rules = vec![crate::config::RuleConfig { target_template: "button.png".to_string() }];
    let prepared_rules = vec![PreparedRule {
        target_template: "button.png".to_string(),
        template_path: std::path::PathBuf::from("button.png"),
        template_size: (24, 12),
        template_mat: Arc::new(to_gray(&exact)),
        template_colors: crate::matcher::mat_color_stats(&exact).unwrap(),
    }];
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 80,
        height: 40,
        origin_x: 0,
        origin_y: 0,
    };
    let mut executor = RecordingExecutor::default();
    let mut cycle = |screen: &Mat| {
        let mut capture = CaptureService::with_source(
            "DP-1",
            ColorScreen(screen_with(80, 40, SCREEN_BACKGROUND, screen, 30, 14)),
        );
        let mut history = CycleHistory::default();
        run_cycle(&rules, &prepared_rules, 0.9, &monitor, &mut capture, &mut executor, &mut history)
    };

    let dimmed_outcome = cycle(&dimmed).unwrap();
    let exact_outcome = cycle(&exact).unwrap();

    assert_eq!(dimmed_outcome, None);
    assert_eq!(exact_outcome.map(|click| (click.output_x, click.output_y)), Some((42, 20)));
    assert_eq!(executor.clicks.len(), 1);
}

#[test]
fn a_template_failing_the_color_check_for_a_sustained_streak_warns_once_and_resets_on_acceptance() {
    use crate::support::{blend_toward, button, map_pixels, screen_with, to_gray};
    use crate::support::{BUTTON_FILL, BUTTON_TEXT, SCREEN_BACKGROUND};
    const WARNING: &str =
        "template button.png keeps matching in grayscale but failing the color check";

    let exact = button(BUTTON_FILL, BUTTON_TEXT);
    let dimmed = map_pixels(&exact, |pixel| blend_toward(pixel, 128, 0.4));
    let rules = vec![crate::config::RuleConfig { target_template: "button.png".to_string() }];
    let prepared_rules = vec![PreparedRule {
        target_template: "button.png".to_string(),
        template_path: std::path::PathBuf::from("button.png"),
        template_size: (24, 12),
        template_mat: Arc::new(to_gray(&exact)),
        template_colors: crate::matcher::mat_color_stats(&exact).unwrap(),
    }];
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 80,
        height: 40,
        origin_x: 0,
        origin_y: 0,
    };
    let mut executor = RecordingExecutor::default();
    let mut history = CycleHistory {
        previous_click: None,
        unclickable: UnclickableStreaks::new(UNCLICKABLE_STREAK_CYCLES, Duration::from_secs(3600)),
    };
    let mut cycles = |screen: &Mat, count: u32| {
        crate::support::capture_debug_logs(|| {
            for _ in 0..count {
                let mut capture = CaptureService::with_source(
                    "DP-1",
                    ColorScreen(screen_with(80, 40, SCREEN_BACKGROUND, screen, 30, 14)),
                );
                let rules = (&rules, &prepared_rules, &monitor);
                run_cycle(rules.0, rules.1, 0.9, rules.2, &mut capture, &mut executor, &mut history)
                    .unwrap();
            }
        })
    };

    let short_streak = cycles(&dimmed, UNCLICKABLE_STREAK_CYCLES - 1);
    let sustained = cycles(&dimmed, 1);
    let rate_limited = cycles(&dimmed, 20);
    let accepted = cycles(&exact, 1);
    let restarted_short = cycles(&dimmed, UNCLICKABLE_STREAK_CYCLES - 1);
    let restarted = cycles(&dimmed, 1);

    assert!(!short_streak.contains(WARNING), "{short_streak}");
    assert_eq!(sustained.matches(WARNING).count(), 1, "{sustained}");
    assert!(sustained.contains(" WARN "), "{sustained}");
    for field in ["target_template=button.png", "cycles=12", "score=", "max_channel_delta="] {
        assert!(sustained.contains(field), "missing {field}: {sustained}");
    }
    assert!(sustained.contains("contrast_ratio=0.6"), "{sustained}");
    assert!(!rate_limited.contains(WARNING), "{rate_limited}");
    assert!(!accepted.contains(WARNING), "{accepted}");
    assert_eq!(executor.clicks.len(), 1);
    // The accepted match ended the streak: a new one needs the full count again.
    assert!(!restarted_short.contains(WARNING), "{restarted_short}");
    assert_eq!(restarted.matches(WARNING).count(), 1, "{restarted}");
}

#[test]
fn a_template_too_large_to_score_warns_after_the_streak_at_most_once_per_interval() {
    const WARNING: &str =
        "template big.png is larger than the captured frame and cannot be matched";
    let rule = PreparedRule {
        target_template: "big.png".to_string(),
        template_path: std::path::PathBuf::from("big.png"),
        template_size: (100, 50),
        template_mat: Arc::new(capture_mat(100, 50)),
        template_colors: ColorStats { mean_bgr: [0.0; 3], luma_std: 1.0 },
    };
    let unscored = crate::matcher::TemplateScores::from([("big.png".to_string(), None)]);
    let not_found = crate::matcher::TemplateScores::from([("big.png".to_string(), Some(0.2))]);
    let no_rejections = crate::matcher::ColorRejections::new();
    let observe = |streaks: &mut UnclickableStreaks, scores, count| {
        crate::support::capture_debug_logs(|| {
            for _ in 0..count {
                streaks.observe(std::slice::from_ref(&rule), scores, &no_rejections, (64, 32));
            }
        })
        .matches(WARNING)
        .count()
    };

    // Without a rate limit every cycle past the streak warns; with one, only
    // the first does.
    let mut unlimited = UnclickableStreaks::new(3, Duration::ZERO);
    let mut limited = UnclickableStreaks::new(3, Duration::from_secs(3600));
    assert_eq!(observe(&mut unlimited, &unscored, 2), 0);
    assert_eq!(observe(&mut unlimited, &unscored, 3), 3);
    assert_eq!(observe(&mut limited, &unscored, 5), 1);
    // A cycle that scores the template, even without finding it, ends the streak.
    assert_eq!(observe(&mut limited, &not_found, 1), 0);
    assert_eq!(observe(&mut limited, &unscored, 2), 0);
    let logs = crate::support::capture_debug_logs(|| {
        limited.observe(std::slice::from_ref(&rule), &unscored, &no_rejections, (64, 32));
    });
    assert!(logs.contains("template_size=100x50 frame_size=64x32"), "{logs}");
}
