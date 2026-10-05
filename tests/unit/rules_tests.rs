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
    let click = evaluate_rules(
        &[crate::config::RuleConfig {
            target_template: "missing.png".to_string(),
        }],
        &match_set(),
        crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 },
        None,
    );

    assert!(click.is_none());
}

#[test]
fn matches_target_template_when_region_exists() {
    let click = evaluate_rules(
        &[crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
        &match_set(),
        crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 },
        None,
    )
    .expect("the rule's template matched");

    assert_eq!(click.rule_index, 0);
    assert_eq!(click.target_template, "accept_button.png");
}

#[test]
fn picks_first_matching_box_only_once_per_rule() {
    let click = evaluate_rules(
        &[crate::config::RuleConfig {
            target_template: "ready_button.png".to_string(),
        }],
        &match_set(),
        crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 },
        None,
    )
    .expect("the rule's template matched");

    assert_eq!(click.output_x, 440);
    assert_eq!(click.output_y, 335);
    assert_eq!(click.extent, crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 });
}

#[test]
fn plans_only_output_local_centers_without_monitor_origin() {
    let point = plan_center_click(&MatchRegion { left: 50, top: 60, width: 101, height: 41 });
    assert_eq!(point, (100, 80));
}

fn two_rules(first: &str, second: &str) -> Vec<crate::config::RuleConfig> {
    vec![
        crate::config::RuleConfig { target_template: first.to_string() },
        crate::config::RuleConfig { target_template: second.to_string() },
    ]
}

#[test]
fn plans_only_the_first_rule_when_several_rules_match() {
    let click = evaluate_rules(
        &two_rules("accept_button.png", "ready_button.png"),
        &match_set(),
        crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 },
        None,
    )
    .expect("both rules matched");

    assert_eq!(click.rule_index, 0);
    assert_eq!(click.target_template, "accept_button.png");
}

#[test]
fn plans_the_first_matching_rule_when_earlier_rules_miss() {
    let click = evaluate_rules(
        &two_rules("missing.png", "ready_button.png"),
        &match_set(),
        crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 },
        None,
    )
    .expect("the second rule matched");

    assert_eq!(click.rule_index, 1);
    assert_eq!(click.target_template, "ready_button.png");
}

#[test]
fn plans_nothing_when_no_rule_matches() {
    let click = evaluate_rules(
        &two_rules("missing.png", "also_missing.png"),
        &match_set(),
        crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 },
        None,
    );

    assert!(click.is_none());
}

fn rules_for(templates: &[&str]) -> Vec<crate::config::RuleConfig> {
    templates
        .iter()
        .map(|template| crate::config::RuleConfig { target_template: template.to_string() })
        .collect()
}

fn matches_for(templates: &[&str]) -> MatchSet {
    templates
        .iter()
        .map(|template| {
            (template.to_string(), vec![MatchRegion { left: 0, top: 0, width: 10, height: 10 }])
        })
        .collect()
}

fn chosen_rule(
    templates: &[&str],
    matched: &[&str],
    previous_click: Option<usize>,
) -> Option<usize> {
    evaluate_rules(
        &rules_for(templates),
        &matches_for(matched),
        crate::wayland_pointer::ImageExtent { width: 2560, height: 1440 },
        previous_click,
    )
    .map(|click| click.rule_index)
}

#[test]
fn uses_config_order_without_a_previous_click() {
    assert_eq!(chosen_rule(&["a.png", "b.png"], &["a.png", "b.png"], None), Some(0));
}

#[test]
fn gives_the_next_matched_rule_a_turn_after_the_previous_click() {
    assert_eq!(chosen_rule(&["a.png", "b.png"], &["a.png", "b.png"], Some(0)), Some(1));
}

#[test]
fn clicks_the_previous_rule_again_when_it_is_the_only_match() {
    assert_eq!(chosen_rule(&["a.png", "b.png"], &["a.png"], Some(0)), Some(0));
}

#[test]
fn wraps_around_to_the_first_rule_after_the_last_one() {
    assert_eq!(chosen_rule(&["a.png", "b.png"], &["a.png", "b.png"], Some(1)), Some(0));
}

#[test]
fn a_third_rule_gets_a_turn_after_the_second() {
    let templates = ["a.png", "b.png", "c.png"];
    assert_eq!(chosen_rule(&templates, &templates, Some(1)), Some(2));
}
