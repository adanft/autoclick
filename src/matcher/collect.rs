use super::MatchRegion;
use anyhow::Result;
use opencv::core::{self, Mat, Point};
use opencv::prelude::*;

/// One template's scan result: the best score OpenCV reported, and the region it
/// belongs to when that score meets the threshold.
///
/// The score leaves this function even when the match is rejected. A threshold
/// set too high fails silently — no match, no error — so the only way to tune it
/// is to see how close the best candidate actually came.
pub(crate) struct TemplateScan {
    /// `None` when nothing was scored: an empty score matrix, or a template the
    /// caller never ran because it does not fit inside the screenshot.
    pub best_score: Option<f64>,
    pub regions: Vec<MatchRegion>,
}

impl TemplateScan {
    /// A template that was never scored against the screenshot.
    pub(crate) fn unscored() -> Self {
        Self {
            best_score: None,
            regions: Vec::new(),
        }
    }
}

/// Collects the highest-scoring match whose score meets or exceeds the configured threshold.
///
/// OpenCV's `minMaxLoc` scans row-major and only replaces the maximum on a strictly
/// higher score, so tied scores resolve to the first candidate in scan order.
pub(crate) fn collect_regions(
    result: &Mat,
    template_size: (u32, u32),
    threshold: f32,
) -> Result<TemplateScan> {
    if result.empty() {
        return Ok(TemplateScan::unscored());
    }

    let mut best_score = 0.0_f64;
    let mut best_location = Point::default();
    core::min_max_loc(
        result,
        None,
        Some(&mut best_score),
        None,
        Some(&mut best_location),
        &core::no_array(),
    )?;

    // The score matrix is CV_32FC1, so widening both sides to f64 is exact.
    // Keep the comparison in the `>=` direction: a NaN maximum must be rejected,
    // and `NaN >= threshold` is false exactly like the element-wise filter this
    // replaced. Testing `best_score < threshold` instead would accept it.
    let accepted = best_score >= f64::from(threshold);
    if !accepted {
        return Ok(TemplateScan {
            best_score: Some(best_score),
            regions: Vec::new(),
        });
    }

    Ok(TemplateScan {
        best_score: Some(best_score),
        regions: vec![MatchRegion {
            left: best_location.x,
            top: best_location.y,
            width: template_size.0 as i32,
            height: template_size.1 as i32,
        }],
    })
}
