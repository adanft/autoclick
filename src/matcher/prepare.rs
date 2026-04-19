use super::engine::{load_grayscale_mat, mat_dimensions};
use super::PreparedRule;
use crate::config::RuleConfig;
use anyhow::{bail, Context, Result};
use opencv::core::Mat;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

        prepared.push(PreparedRule {
            target_template: rule.target_template.clone(),
            template_path,
            template_size,
            template_mat,
        });
    }

    Ok(prepared)
}
