mod collect;
mod engine;
mod prepare;

use opencv::core::Mat;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub use engine::scan_all;
pub use prepare::prepare_rules;

/// Prepared runtime representation of one configured template rule.
///
/// The template image is decoded once and shared across duplicate rules via
/// `Arc<Mat>` so the runtime loop avoids repeated disk I/O and decoding work.
#[derive(Debug, Clone)]
pub struct PreparedRule {
    pub target_template: String,
    pub template_path: PathBuf,
    pub template_size: (u32, u32),
    pub template_mat: Arc<Mat>,
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
