use super::collect::{collect_regions, TemplateScan};
use super::{MatchSet, PreparedRule};
use anyhow::{bail, Context, Result};
use opencv::core::{Mat, Rect, Vec3b, CV_8UC1, CV_8UC3};
use opencv::{imgcodecs, imgproc, prelude::*};
use std::collections::BTreeSet;
use std::path::Path;
use tracing::debug;

/// Largest difference, on any one channel, between the mean color of a template
/// and of its matched region, out of 255.
///
/// Grayscale matching ignores color, so this is what rejects a recolored or
/// desaturated copy of a button: on the synthetic test button those differ by
/// 137 and 57. A hover highlight lightening the button 12% shifts it by 19, and
/// even lightening pure black 15% toward white moves it only 38, so 40 still
/// tolerates the highlight the virtual pointer leaves on a button it just
/// clicked.
pub const MAX_MEAN_CHANNEL_DELTA: f64 = 40.0;

/// Lowest accepted ratio of the region's luminance spread to the template's.
///
/// Normalized matching also ignores contrast, and a button dimmed toward gray
/// keeps its hue, so a dimmed or disabled copy shows up here first: blending 40%
/// toward mid-gray leaves 0.60 of the spread while its mean color moves only
/// 26. A 12% hover highlight keeps 0.88 (0.85 at 15%), so 0.75 sits between.
pub const MIN_CONTRAST_RATIO: f64 = 0.75;

/// Highest accepted ratio of the region's luminance spread to the template's,
/// the reciprocal of [`MIN_CONTRAST_RATIO`], so a template captured from a
/// dimmed button rejects the bright one just as the opposite case does.
pub const MAX_CONTRAST_RATIO: f64 = 1.0 / MIN_CONTRAST_RATIO;

/// Mean color and luminance spread of an image area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorStats {
    /// Mean blue, green and red, each out of 255.
    pub mean_bgr: [f64; 3],
    /// Population standard deviation of the BT.601 luma OpenCV's `BGR2GRAY`
    /// computes, so it measures the contrast the grayscale matcher saw.
    pub luma_std: f64,
}

/// How a matched region's colors differ from its template's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorComparison {
    /// Region minus template mean, per blue, green and red channel.
    pub channel_deltas: [f64; 3],
    /// Region luminance spread divided by the template's.
    pub contrast_ratio: f64,
}

impl ColorComparison {
    pub fn between(template: &ColorStats, region: &ColorStats) -> Self {
        let channel_deltas =
            [0, 1, 2].map(|channel| region.mean_bgr[channel] - template.mean_bgr[channel]);
        // `prepare_rules` refuses flat templates, but a flat one must still only
        // accept an equally flat region rather than divide by zero.
        let contrast_ratio = if template.luma_std > 0.0 {
            region.luma_std / template.luma_std
        } else if region.luma_std > 0.0 {
            f64::INFINITY
        } else {
            1.0
        };
        Self {
            channel_deltas,
            contrast_ratio,
        }
    }

    /// The largest absolute per-channel mean difference.
    pub fn max_channel_delta(&self) -> f64 {
        self.channel_deltas
            .iter()
            .fold(0.0, |max, delta| f64::max(max, delta.abs()))
    }

    /// Whether the region looks like the template in color and contrast. A NaN
    /// ratio fails the range check and is rejected.
    pub fn accepted(&self) -> bool {
        self.max_channel_delta() <= MAX_MEAN_CHANNEL_DELTA
            && (MIN_CONTRAST_RATIO..=MAX_CONTRAST_RATIO).contains(&self.contrast_ratio)
    }
}

/// Accumulates [`ColorStats`] one BGR pixel at a time.
#[derive(Default)]
struct ColorAccumulator {
    channel_sums: [u64; 3],
    luma_sum: u64,
    luma_square_sum: u64,
    pixels: u64,
}

impl ColorAccumulator {
    fn add(&mut self, [blue, green, red]: [u8; 3]) {
        for (sum, channel) in self.channel_sums.iter_mut().zip([blue, green, red]) {
            *sum += u64::from(channel);
        }
        // OpenCV's fixed-point BT.601 weights and rounding, so the luma matches
        // the grayscale pixel `cvtColor` and `IMREAD_GRAYSCALE` produce.
        let luma =
            (u64::from(red) * 4899 + u64::from(green) * 9617 + u64::from(blue) * 1868 + (1 << 13))
                >> 14;
        self.luma_sum += luma;
        self.luma_square_sum += luma * luma;
        self.pixels += 1;
    }

    fn finish(&self) -> Option<ColorStats> {
        if self.pixels == 0 {
            return None;
        }
        let count = self.pixels as f64;
        let luma_mean = self.luma_sum as f64 / count;
        // Squares are summed exactly in integers, so only this final step rounds.
        let variance = self.luma_square_sum as f64 / count - luma_mean * luma_mean;
        Some(ColorStats {
            mean_bgr: self.channel_sums.map(|sum| sum as f64 / count),
            luma_std: variance.max(0.0).sqrt(),
        })
    }
}

/// Color statistics of BGR pixels, or `None` when there are none.
pub fn bgr_color_stats(pixels: impl IntoIterator<Item = [u8; 3]>) -> Option<ColorStats> {
    let mut accumulator = ColorAccumulator::default();
    pixels.into_iter().for_each(|pixel| accumulator.add(pixel));
    accumulator.finish()
}

/// Color statistics of an 8-bit BGR matrix, or of a grayscale one read as equal
/// blue, green and red. Region views work without copying them first.
pub fn mat_color_stats(image: &impl MatTraitConst) -> Result<ColorStats> {
    let mut accumulator = ColorAccumulator::default();
    for row in 0..image.rows() {
        match image.typ() {
            CV_8UC3 => image
                .at_row::<Vec3b>(row)?
                .iter()
                .for_each(|pixel| accumulator.add(pixel.0)),
            CV_8UC1 => image
                .at_row::<u8>(row)?
                .iter()
                .for_each(|&gray| accumulator.add([gray; 3])),
            other => bail!("color statistics need an 8-bit gray or BGR image, got type {other}"),
        }
    }
    accumulator
        .finish()
        .context("color statistics need at least one pixel")
}

/// Drops each template's best match whose colors differ from the template's.
///
/// Grayscale `TM_CCOEFF_NORMED` matching ignores brightness, contrast and hue,
/// so a dimmed, disabled or recolored copy of a button scores like the real one.
/// `sample` returns the color statistics of a screen rectangle; it is called
/// once per matched template, on its best region only. A rejected template is
/// left unmatched for this cycle rather than falling back to a weaker candidate.
pub fn verify_colors<F>(matches: &mut MatchSet, rules: &[PreparedRule], mut sample: F) -> Result<()>
where
    F: FnMut(Rect) -> Result<ColorStats>,
{
    let mut checked = BTreeSet::new();
    for rule in rules {
        if !checked.insert(rule.target_template.as_str()) {
            continue;
        }
        let Some(regions) = matches.get_mut(&rule.target_template) else {
            continue;
        };
        let Some(best) = regions.first() else {
            continue;
        };
        let region =
            sample(Rect::new(best.left, best.top, best.width, best.height)).with_context(|| {
                format!(
                    "failed to sample the screen colors matched by `{}`",
                    rule.target_template
                )
            })?;
        let comparison = ColorComparison::between(&rule.template_colors, &region);
        let [delta_blue, delta_green, delta_red] = comparison.channel_deltas;
        if comparison.accepted() {
            debug!(
                target_template = %rule.target_template,
                max_channel_delta = comparison.max_channel_delta(),
                contrast_ratio = comparison.contrast_ratio,
                "color check accepted match"
            );
            continue;
        }
        debug!(
            target_template = %rule.target_template,
            max_channel_delta = comparison.max_channel_delta(),
            delta_blue,
            delta_green,
            delta_red,
            contrast_ratio = comparison.contrast_ratio,
            max_allowed_delta = MAX_MEAN_CHANNEL_DELTA,
            min_contrast_ratio = MIN_CONTRAST_RATIO,
            max_contrast_ratio = MAX_CONTRAST_RATIO,
            "color check rejected match"
        );
        regions.clear();
    }
    Ok(())
}

/// Runs OpenCV template matching for every configured rule against one screenshot.
///
/// The returned regions contain only the single best candidate that meets the
/// threshold for each template.
pub fn scan_all(screenshot_mat: &Mat, rules: &[PreparedRule], threshold: f32) -> Result<MatchSet> {
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

        let scan = if rule.template_mat.cols() > screenshot_mat.cols()
            || rule.template_mat.rows() > screenshot_mat.rows()
        {
            TemplateScan::unscored()
        } else {
            let result =
                run_match_template(screenshot_mat, &rule.template_mat).with_context(|| {
                    format!("OpenCV matchTemplate failed for `{}`", rule.target_template)
                })?;
            collect_regions(&result, rule.template_size, threshold)?
        };
        // The score is logged for rejected templates too: a threshold set above
        // what the screen actually produces is otherwise indistinguishable from
        // a template that is simply not on screen.
        debug!(
            target_template = %rule.target_template,
            score = scan.best_score,
            threshold,
            candidates = scan.regions.len(),
            "OpenCV matcher finished template scan"
        );

        matches.insert(rule.target_template.clone(), scan.regions);
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

/// Loads a template as the grayscale matrix the matcher scans with, plus the
/// color statistics of its BGR pixels for the check after a match.
pub(crate) fn load_template(path: &Path) -> Result<(Mat, ColorStats)> {
    let gray = load_grayscale_mat(path)?;
    let color = imgcodecs::imread(&path.to_string_lossy(), imgcodecs::IMREAD_COLOR)
        .context("OpenCV could not load image from disk in color")?;
    if color.empty() {
        bail!("OpenCV returned an empty color image");
    }
    Ok((gray, mat_color_stats(&color)?))
}

/// Returns the standard deviation of a single-channel image's pixel values.
pub(crate) fn template_stddev(mat: &Mat) -> Result<f64> {
    let mut mean = Mat::default();
    let mut stddev = Mat::default();
    opencv::core::mean_std_dev(mat, &mut mean, &mut stddev, &opencv::core::no_array())
        .context("OpenCV meanStdDev execution failed")?;

    stddev
        .at_2d::<f64>(0, 0)
        .copied()
        .context("OpenCV meanStdDev returned no deviation channel")
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
///
/// `TM_CCOEFF_NORMED` subtracts the mean of both images before correlating, so a
/// uniformly bright screen region cannot score high against an unrelated template
/// the way it can under `TM_CCORR_NORMED`.
fn run_match_template(screenshot: &Mat, template: &Mat) -> Result<Mat> {
    let mut result = Mat::default();
    imgproc::match_template(
        screenshot,
        template,
        &mut result,
        imgproc::TM_CCOEFF_NORMED,
        &Mat::default(),
    )
    .context("OpenCV matchTemplate execution failed")?;
    Ok(result)
}
