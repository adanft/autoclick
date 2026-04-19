use super::MatchRegion;
use anyhow::Result;
use opencv::core::Mat;
use opencv::prelude::*;

/// Collects the highest-scoring match whose score meets or exceeds the configured threshold.
pub(crate) fn collect_regions(
    result: &Mat,
    template_size: (u32, u32),
    threshold: f32,
) -> Result<Vec<MatchRegion>> {
    let mut best_match: Option<(f32, MatchRegion)> = None;

    for top in 0..result.rows() {
        for left in 0..result.cols() {
            let score = *result.at_2d::<f32>(top, left)?;
            if score >= threshold {
                let region = MatchRegion {
                    left,
                    top,
                    width: template_size.0 as i32,
                    height: template_size.1 as i32,
                };

                let should_replace = best_match
                    .as_ref()
                    .map(|(best_score, _)| score > *best_score)
                    .unwrap_or(true);

                if should_replace {
                    best_match = Some((score, region));
                }
            }
        }
    }

    Ok(best_match.into_iter().map(|(_, region)| region).collect())
}
