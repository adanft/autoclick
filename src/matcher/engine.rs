use super::collect::collect_regions;
use super::{MatchSet, PreparedRule};
use anyhow::{bail, Context, Result};
use opencv::core::Mat;
use opencv::{imgcodecs, imgproc, prelude::*};
use std::path::Path;
use tracing::debug;

/// Runs OpenCV template matching for every configured rule against one screenshot.
///
/// The returned regions contain only the single best candidate that meets the
/// threshold for each template.
pub fn scan_all(screenshot: &Path, rules: &[PreparedRule], threshold: f32) -> Result<MatchSet> {
    let screenshot_mat = load_grayscale_mat(screenshot)
        .with_context(|| format!("failed to decode screenshot {}", screenshot.display()))?;
    let mut matches = MatchSet::new();

    for rule in rules {
        if matches.contains_key(&rule.target_template) {
            continue;
        }

        debug!(
            target_template = %rule.target_template,
            threshold,
            template_width = rule.template_size.0,
            template_height = rule.template_size.1,
            screenshot_width = screenshot_mat.cols(),
            screenshot_height = screenshot_mat.rows(),
            "OpenCV matcher scanning template"
        );

        let regions = if rule.template_mat.cols() > screenshot_mat.cols()
            || rule.template_mat.rows() > screenshot_mat.rows()
        {
            Vec::new()
        } else {
            let result =
                run_match_template(&screenshot_mat, &rule.template_mat).with_context(|| {
                    format!(
                        "OpenCV matchTemplate failed for `{}` using screenshot {}",
                        rule.target_template,
                        screenshot.display()
                    )
                })?;
            collect_regions(&result, rule.template_size, threshold)?
        };
        debug!(target_template = %rule.target_template, candidates = regions.len(), "OpenCV matcher finished template scan");

        matches.insert(rule.target_template.clone(), regions);
    }

    Ok(matches)
}

/// Loads an image as a non-empty grayscale OpenCV matrix.
pub(crate) fn load_grayscale_mat(path: &Path) -> Result<Mat> {
    let path = path.to_string_lossy();
    let mat = imgcodecs::imread(&path, imgcodecs::IMREAD_GRAYSCALE)
        .context("OpenCV could not load image from disk")?;

    if mat.empty() {
        bail!("OpenCV returned an empty image");
    }

    Ok(mat)
}

/// Returns image dimensions while rejecting empty or invalid matrices.
pub(crate) fn mat_dimensions(mat: &Mat) -> Result<(u32, u32)> {
    let width = mat.cols();
    let height = mat.rows();

    if width <= 0 || height <= 0 {
        bail!("OpenCV image dimensions must be greater than zero");
    }

    Ok((width as u32, height as u32))
}

/// Executes OpenCV `matchTemplate` with the current normalized correlation mode.
fn run_match_template(screenshot: &Mat, template: &Mat) -> Result<Mat> {
    let mut result = Mat::default();
    imgproc::match_template(
        screenshot,
        template,
        &mut result,
        imgproc::TM_CCORR_NORMED,
        &Mat::default(),
    )
    .context("OpenCV matchTemplate execution failed")?;
    Ok(result)
}
