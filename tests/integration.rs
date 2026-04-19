use autoclick::config::RuleConfig;
use autoclick::matcher::{MatchRegion, MatchSet};
use autoclick::monitor::MonitorSpec;
use autoclick::runtime::execute_match_set;

#[test]
fn executes_clicks_for_multiple_rules_in_order_through_public_api() {
    let monitor = MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 100,
        origin_y: 200,
    };
    let rules = vec![
        RuleConfig {
            target_template: "accept_button.png".to_string(),
        },
        RuleConfig {
            target_template: "ready_button.png".to_string(),
        },
    ];
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
                left: 30,
                top: 40,
                width: 20,
                height: 10,
            }],
        ),
    ]);

    let mut clicks = Vec::new();
    let planned = execute_match_set(&rules, &monitor, &matches, |x, y| {
        clicks.push((x, y));
        Ok(())
    })
    .unwrap();

    assert_eq!(planned.len(), 2);
    assert_eq!(planned[0].rule_index, 0);
    assert_eq!(planned[1].rule_index, 1);
    assert_eq!(clicks, vec![(120, 225), (140, 245)]);
}
