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
    run_monitor_loop_with_runner(60_000, shutdown_rx, || {
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
fn stops_after_too_many_consecutive_non_click_failures() {
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut calls = 0_usize;

    let error = run_monitor_loop_with_runner(1, rx, || {
        calls += 1;
        if calls == MAX_CONSECUTIVE_CYCLE_FAILURES {
            return Err(capture_failure("Wayland output HDMI-A-1 was removed"));
        }
        Err(match_failure("temporary matcher failure"))
    })
    .unwrap_err();

    assert_eq!(calls, MAX_CONSECUTIVE_CYCLE_FAILURES);
    assert_eq!(
        format!("{error:#}"),
        format!(
            "monitor loop stopped after {MAX_CONSECUTIVE_CYCLE_FAILURES} consecutive failed \
             cycles, last in stage capture: capture failed: Wayland output HDMI-A-1 was removed"
        )
    );
}

#[test]
fn a_successful_cycle_resets_the_consecutive_failure_count() {
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut calls = 0_usize;

    run_monitor_loop_with_runner(1, rx, || {
        calls += 1;
        // One short of the limit, then a success, then failures until it stops.
        if calls == MAX_CONSECUTIVE_CYCLE_FAILURES {
            return Ok(());
        }
        Err(match_failure("temporary matcher failure"))
    })
    .unwrap_err();

    assert_eq!(calls, 2 * MAX_CONSECUTIVE_CYCLE_FAILURES);
}

#[test]
fn click_failure_stops_immediately_even_after_earlier_skipped_cycles() {
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut calls = 0_usize;

    let error = run_monitor_loop_with_runner(1, rx, || {
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

    run_monitor_loop_with_runner(1, rx, move || {
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

    let error = run_monitor_loop_with_runner(1, rx, move || {
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

    run_monitor_loop_with_runner(1, rx, move || {
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
