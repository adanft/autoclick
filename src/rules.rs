use crate::config::RuleConfig;
use crate::matcher::{MatchRegion, MatchSet};
use crate::monitor::MonitorSpec;
use serde::{Deserialize, Serialize};

/// Click request derived from one configured rule and one accepted match.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedClick {
    pub rule_index: usize,
    pub target_template: String,
    pub abs_x: i32,
    pub abs_y: i32,
}

/// Evaluates rules in configuration order and produces click plans for the first
/// accepted match of each rule.
pub fn evaluate_rules(
    rules: &[RuleConfig],
    matches: &MatchSet,
    monitor: &MonitorSpec,
) -> Vec<PlannedClick> {
    rules
        .iter()
        .enumerate()
        .filter_map(|(rule_index, rule)| evaluate_rule(rule_index, rule, matches, monitor))
        .collect()
}

fn evaluate_rule(
    rule_index: usize,
    rule: &RuleConfig,
    matches: &MatchSet,
    monitor: &MonitorSpec,
) -> Option<PlannedClick> {
    let matching_region = matches.get(&rule.target_template)?.first()?;
    let (abs_x, abs_y) = plan_center_click(monitor, matching_region);
    Some(PlannedClick {
        rule_index,
        target_template: rule.target_template.clone(),
        abs_x,
        abs_y,
    })
}

/// Converts a match region into absolute screen coordinates for a centered click.
pub fn plan_center_click(monitor: &MonitorSpec, region: &MatchRegion) -> (i32, i32) {
    let center_x = monitor.origin_x + region.left + (region.width / 2);
    let center_y = monitor.origin_y + region.top + (region.height / 2);
    (center_x, center_y)
}
