fn match_set() -> MatchSet {
    MatchSet::from([
        (
            "accept_button.png".to_string(),
            vec![MatchRegion {
                left: 100,
                top: 200,
                width: 60,
                height: 20,
            }],
        ),
        (
            "ready_button.png".to_string(),
            vec![
                MatchRegion {
                    left: 400,
                    top: 320,
                    width: 80,
                    height: 30,
                },
                MatchRegion {
                    left: 500,
                    top: 320,
                    width: 80,
                    height: 30,
                },
            ],
        ),
    ])
}

fn monitor() -> crate::monitor::MonitorSpec {
    crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 1920,
        origin_y: 0,
    }
}

#[test]
fn ignores_rules_without_a_match() {
    let clicks = evaluate_rules(
        &[crate::config::RuleConfig {
            target_template: "missing.png".to_string(),
        }],
        &match_set(),
        crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 },
    );

    assert!(clicks.is_empty());
}

#[test]
fn matches_target_template_when_region_exists() {
    let clicks = evaluate_rules(
        &[crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
        &match_set(),
        crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 },
    );

    assert_eq!(clicks.len(), 1);
    assert_eq!(clicks[0].rule_index, 0);
    assert_eq!(clicks[0].target_template, "accept_button.png");
}

#[test]
fn picks_first_matching_box_only_once_per_rule() {
    let clicks = evaluate_rules(
        &[crate::config::RuleConfig {
            target_template: "ready_button.png".to_string(),
        }],
        &match_set(),
        crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 },
    );

    assert_eq!(clicks.len(), 1);
        assert_eq!(clicks[0].output_x, 440);
        assert_eq!(clicks[0].output_y, 335);
        assert_eq!(
            clicks[0].extent,
            crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 }
        );
}

#[test]
fn plans_only_output_local_centers_without_monitor_origin() {
    let point = plan_center_click(&MatchRegion { left: 50, top: 60, width: 101, height: 41 });
    assert_eq!(point, (100, 80));
}
