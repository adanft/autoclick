mod collect;
mod engine;
mod prepare;

use opencv::core::Mat;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub use engine::{
    bgr_color_stats, mat_color_stats, scan_all, scan_all_scored, verify_colors, ColorComparison,
    ColorRejections, ColorStats, TemplateScores, MAX_CONTRAST_RATIO, MAX_MEAN_CHANNEL_DELTA,
    MIN_CONTRAST_RATIO,
};
pub use prepare::prepare_rules;

/// Prepared runtime representation of one configured template rule.
///
/// The template image is decoded once at startup and held behind `Arc<Mat>` so
/// the runtime loop avoids repeated disk I/O and decoding work. Its color
/// statistics are measured at the same time for the check after a match.
#[derive(Debug, Clone)]
pub struct PreparedRule {
    pub target_template: String,
    pub template_path: PathBuf,
    pub template_size: (u32, u32),
    pub template_mat: Arc<Mat>,
    pub template_colors: ColorStats,
}

/// Rectangle reported by the OpenCV matcher in screenshot-local coordinates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatchRegion {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

/// Match results grouped by the configured template name.
pub type MatchSet = BTreeMap<String, Vec<MatchRegion>>;
