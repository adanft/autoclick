use crate::config::RuleConfig;
use crate::matcher::{MatchRegion, MatchSet};
use crate::wayland_pointer::ImageExtent;
pub use crate::wayland_pointer::PlannedClick;
use tracing::debug;

/// Plans at most one click per cycle, taking turns among the matched rules.
///
/// Every planned click comes from the same screenshot, so a second click would
/// aim at where its target was before the first click changed the screen.
/// Other rules that also matched are deferred to the next cycle's fresh capture.
///
/// `previous_click` is the rule index clicked in the previous cycle. The search
/// starts right after it and wraps around, so that rule comes last: a target
/// that stays on screen after its click cannot starve the other matched rules.
/// Without a previous click the search follows configuration order.
pub fn evaluate_rules(
    rules: &[RuleConfig],
    matches: &MatchSet,
    extent: ImageExtent,
    previous_click: Option<usize>,
) -> Option<PlannedClick> {
    let start = previous_click.map_or(0, |index| index + 1);
    let chosen = (0..rules.len())
        .map(|offset| (start + offset) % rules.len())
        .find_map(|rule_index| evaluate_rule(rule_index, &rules[rule_index], matches, extent))?;
    let skipped_previous_rule = previous_click.is_some_and(|index| {
        index != chosen.rule_index
            && rules
                .get(index)
                .is_some_and(|rule| rule_matched(rule, matches))
    });
    debug!(
        rule_index = chosen.rule_index + 1,
        target_template = %chosen.target_template,
        skipped_previous_rule,
        "chose the matched rule for this cycle"
    );
    Some(chosen)
}

fn rule_matched(rule: &RuleConfig, matches: &MatchSet) -> bool {
    matches
        .get(&rule.target_template)
        .is_some_and(|regions| !regions.is_empty())
}

fn evaluate_rule(
    rule_index: usize,
    rule: &RuleConfig,
    matches: &MatchSet,
    extent: ImageExtent,
) -> Option<PlannedClick> {
    let matching_region = matches.get(&rule.target_template)?.first()?;
    let (output_x, output_y) = plan_output_local_center(matching_region);
    Some(PlannedClick {
        rule_index,
        target_template: rule.target_template.clone(),
        output_x,
        output_y,
        extent,
    })
}

/// Converts a match region into output-local coordinates for a centered click.
pub fn plan_center_click(region: &MatchRegion) -> (i32, i32) {
    plan_output_local_center(region)
}

/// Computes the match center in the captured image's output-local coordinate space.
pub fn plan_output_local_center(region: &MatchRegion) -> (i32, i32) {
    (
        region.left + (region.width / 2),
        region.top + (region.height / 2),
    )
}
