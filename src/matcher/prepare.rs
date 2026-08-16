use super::engine::{load_grayscale_mat, mat_dimensions, template_stddev};
use super::PreparedRule;
use crate::config::RuleConfig;
use anyhow::{bail, Context, Result};
use opencv::core::Mat;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Cutoff separating a perfectly uniform template from one carrying structure.
///
/// An 8-bit template of `n` pixels with a single pixel differing by one level has
/// a standard deviation near `1/sqrt(n)`, so at any plausible template size this
/// admits every image that is not entirely one color.
const MIN_TEMPLATE_STDDEV: f64 = 1e-3;

/// Resolves template assets from disk and prepares them for runtime matching.
pub fn prepare_rules(rules: &[RuleConfig], templates_dir: &Path) -> Result<Vec<PreparedRule>> {
    prepare_rules_with_loader(rules, templates_dir, load_grayscale_mat)
}

pub(crate) fn prepare_rules_with_loader<F>(
    rules: &[RuleConfig],
    templates_dir: &Path,
    mut load: F,
) -> Result<Vec<PreparedRule>>
where
    F: FnMut(&Path) -> Result<Mat>,
{
    let mut cache = BTreeMap::<PathBuf, Arc<Mat>>::new();
    let mut prepared = Vec::with_capacity(rules.len());

    for rule in rules {
        let template_path = templates_dir.join(&rule.target_template);
        if !template_path.exists() {
            bail!(
                "template asset `{}` was not found at {}",
                rule.target_template,
                template_path.display()
            );
        }

        let template_mat = match cache.get(&template_path) {
            Some(mat) => Arc::clone(mat),
            None => {
                let mat = Arc::new(load(&template_path).with_context(|| {
                    format!(
                        "template asset `{}` could not be read from {}",
                        rule.target_template,
                        template_path.display()
                    )
                })?);
                cache.insert(template_path.clone(), Arc::clone(&mat));
                mat
            }
        };

        let template_size = mat_dimensions(&template_mat)?;
        reject_uniform_template(&template_mat, &rule.target_template)?;

        prepared.push(PreparedRule {
            target_template: rule.target_template.clone(),
            template_path,
            template_size,
            template_mat,
        });
    }

    Ok(prepared)
}

/// Rejects a template whose pixels are all the same value.
///
/// Normalized template matching cannot discriminate on such an image: OpenCV's
/// degenerate-denominator guard scores every window at 1.0, so the matcher would
/// report a perfect hit at the top-left of every screenshot and the runtime would
/// click there forever. Failing at startup is the only useful outcome.
fn reject_uniform_template(template_mat: &Mat, target_template: &str) -> Result<()> {
    let stddev = template_stddev(template_mat).with_context(|| {
        format!("could not measure the pixel spread of template `{target_template}`")
    })?;

    if stddev < MIN_TEMPLATE_STDDEV {
        bail!(
            "template asset `{target_template}` is a single uniform color; \
             normalized matching would report a perfect match everywhere. \
             Crop a template that includes some contrast."
        );
    }

    Ok(())
}
