use crate::config::RuleConfig;
use crate::matcher::{MatchRegion, MatchSet};
use crate::wayland_pointer::ImageExtent;
pub use crate::wayland_pointer::PlannedClick;

/// Evaluates rules in configuration order and produces click plans for the first
/// accepted match of each rule.
pub fn evaluate_rules(
    rules: &[RuleConfig],
    matches: &MatchSet,
    extent: ImageExtent,
) -> Vec<PlannedClick> {
    rules
        .iter()
        .enumerate()
        .filter_map(|(rule_index, rule)| evaluate_rule(rule_index, rule, matches, extent))
        .collect()
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
