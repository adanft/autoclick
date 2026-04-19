use crate::config::AppConfig;
use crate::matcher::PreparedRule;
use crate::monitor::MonitorSpec;
use std::env;
use std::path::Path;

/// Renders a human-readable summary of the effective runtime configuration.
pub(crate) fn render_startup_summary(
    config: &AppConfig,
    prepared_rules: &[PreparedRule],
    monitor: &MonitorSpec,
    ydotool_owned: bool,
) -> String {
    let mut lines = vec![
        "startup summary".to_string(),
        "  Runtime".to_string(),
        format!(
            "    * Monitor:   {} · {}x{} @ ({}, {})",
            monitor.name, monitor.width, monitor.height, monitor.origin_x, monitor.origin_y
        ),
        format!("    * Interval:  {} ms", config.interval_ms),
        format!("    * Threshold: {:.2}", config.match_threshold),
        "    * Matcher:   OpenCV template matching".to_string(),
        format!(
            "    * Ydotoold:  {}",
            if ydotool_owned { "managed" } else { "external" }
        ),
        String::new(),
        format!("  Rules ({})", config.rules.len()),
    ];

    for (index, rule) in config.rules.iter().enumerate() {
        let prepared = &prepared_rules[index];
        lines.push(format!("    {}. {}", index + 1, rule.target_template));
        lines.push(format!(
            "       - asset: {}",
            display_asset_path(&prepared.template_path)
        ));
        lines.push(format!(
            "       - size:  {}x{}",
            prepared.template_size.0, prepared.template_size.1
        ));
    }
    lines.join("\n") + "\n"
}

fn display_asset_path(path: &Path) -> String {
    let Some(home) = env::var_os("HOME") else {
        return path.display().to_string();
    };

    let home = Path::new(&home);
    match path.strip_prefix(home) {
        Ok(relative) => {
            if relative.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", relative.display())
            }
        }
        Err(_) => path.display().to_string(),
    }
}
