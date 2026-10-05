use autoclick::config::RuleConfig;
use autoclick::matcher::{MatchRegion, MatchSet};
use autoclick::runtime::execute_match_set;
use autoclick::wayland_pointer::{ClickExecutor, ImageExtent, PlannedClick};

struct RecordingExecutor(Vec<PlannedClick>);
impl ClickExecutor for RecordingExecutor {
    fn click(&mut self, click: &PlannedClick) -> anyhow::Result<()> {
        self.0.push(click.clone());
        Ok(())
    }
}
#[test]
fn clicks_only_the_first_matching_rule_through_public_api() {
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
    let mut executor = RecordingExecutor(Vec::new());
    let planned = execute_match_set(
        &rules,
        ImageExtent {
            width: 1920,
            height: 1080,
        },
        &matches,
        None,
        &mut executor,
    )
    .unwrap();
    let click = planned.expect("the first rule matched");
    assert_eq!(executor.0, vec![click.clone()]);
    assert_eq!(
        (click.rule_index, click.output_x, click.output_y),
        (0, 20, 25)
    );
}
