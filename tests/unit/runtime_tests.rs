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

#[test]
fn evaluates_multiple_rules_from_same_match_set_in_order() {
    let monitor = crate::monitor::MonitorSpec {
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

    let planned = crate::rules::evaluate_rules(&rules, &matches, &monitor);

    assert_eq!(planned.len(), 2);
    assert_eq!(planned[0].rule_index, 0);
    assert_eq!(planned[1].rule_index, 1);
}

#[test]
fn loop_stops_when_shutdown_signal_arrives() {
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(()).unwrap();

    match rx.recv_timeout(std::time::Duration::from_millis(1)) {
        Ok(()) => {}
        other => panic!("expected immediate shutdown signal, got {other:?}"),
    }
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
        || Err(anyhow!("grim missing")),
        |_, _| Ok(MatchSet::new()),
        |_| Ok(Vec::new()),
    )
    .unwrap_err();

    assert_eq!(error.stage_label(), "capture");
    assert!(error.to_string().contains("grim missing"));
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
        || Ok(std::path::PathBuf::from("capture.png")),
        |_, _| Err(anyhow!("OpenCV blew up")),
        |_| Ok(Vec::new()),
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
        || Ok(std::path::PathBuf::from("capture.png")),
        |_, _| Ok(MatchSet::new()),
        |_| Err(anyhow!("ydotool socket down")),
    )
    .unwrap_err();

    assert_eq!(error.stage_label(), "click execution");
    assert!(error.to_string().contains("ydotool socket down"));
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
fn run_cycle_reuses_single_capture_and_single_match_pass_for_all_rules() {
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
    let clicks = Arc::new(Mutex::new(Vec::new()));
    let clicks_for_executor = Arc::clone(&clicks);
    let capture_calls = Arc::new(Mutex::new(0_usize));
    let capture_calls_for_closure = Arc::clone(&capture_calls);
    let match_calls = Arc::new(Mutex::new(0_usize));
    let match_calls_for_closure = Arc::clone(&match_calls);
    let rules_for_execution = rules.clone();
    let monitor_for_execution = monitor.clone();

    let planned = run_cycle_with(
        &rules,
        &prepared_rules,
        0.95,
        &monitor,
        move || {
            *capture_calls_for_closure.lock().unwrap() += 1;
            Ok(std::path::PathBuf::from("capture.png"))
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
        move |matches| {
            execute_match_set(
                &rules_for_execution,
                &monitor_for_execution,
                matches,
                move |x, y| {
                    clicks_for_executor.lock().unwrap().push((x, y));
                    Ok(())
                },
            )
        },
    )
    .unwrap();

    assert_eq!(
        planned,
        vec![
            crate::rules::PlannedClick {
                rule_index: 0,
                target_template: "accept_button.png".to_string(),
                abs_x: 120,
                abs_y: 225,
            },
            crate::rules::PlannedClick {
                rule_index: 1,
                target_template: "ready_button.png".to_string(),
                abs_x: 140,
                abs_y: 245,
            },
        ]
    );
    assert_eq!(*clicks.lock().unwrap(), vec![(120, 225), (140, 245)]);
    assert_eq!(*capture_calls.lock().unwrap(), 1);
    assert_eq!(*match_calls.lock().unwrap(), 1);
}

#[test]
fn execute_match_set_invokes_click_executor_with_planned_coordinates() {
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 100,
        origin_y: 200,
    };
    let rules = vec![crate::config::RuleConfig {
        target_template: "accept_button.png".to_string(),
    }];
    let matches = MatchSet::from([(
        "accept_button.png".to_string(),
        vec![MatchRegion {
            left: 10,
            top: 20,
            width: 20,
            height: 10,
        }],
    )]);
    let clicks = Arc::new(Mutex::new(Vec::new()));
    let clicks_for_executor = Arc::clone(&clicks);

    let planned = execute_match_set(&rules, &monitor, &matches, move |x, y| {
        clicks_for_executor.lock().unwrap().push((x, y));
        Ok(())
    })
    .unwrap();

    assert_eq!(
        planned,
        vec![crate::rules::PlannedClick {
            rule_index: 0,
            target_template: "accept_button.png".to_string(),
            abs_x: 120,
            abs_y: 225,
        }]
    );
    assert_eq!(*clicks.lock().unwrap(), vec![(120, 225)]);
}
