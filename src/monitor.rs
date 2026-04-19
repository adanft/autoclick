use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;

/// Display metadata needed to capture and click within a monitor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonitorSpec {
    pub index: usize,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub origin_x: i32,
    pub origin_y: i32,
}

impl MonitorSpec {
    /// Returns a short numbered line suitable for interactive monitor selection.
    pub fn summary_line(&self) -> String {
        format!(
            "{} - {} - {}x{}",
            self.index, self.name, self.width, self.height
        )
    }
}

/// Enumerates monitors using `hyprctl monitors -j`.
pub fn enumerate_monitors() -> Result<Vec<MonitorSpec>> {
    let hyprctl = Command::new("hyprctl").arg("monitors").arg("-j").output();
    if let Ok(output) = hyprctl {
        if output.status.success() {
            return parse_hyprctl(&String::from_utf8_lossy(&output.stdout));
        }
    }

    bail!("unable to enumerate monitors: expected `hyprctl monitors -j`")
}

fn parse_hyprctl(raw: &str) -> Result<Vec<MonitorSpec>> {
    let value: Value = serde_json::from_str(raw).context("invalid hyprctl JSON")?;
    let monitors = value
        .as_array()
        .ok_or_else(|| anyhow!("hyprctl output must be an array"))?;

    let mut result = Vec::new();
    for monitor in monitors {
        let disabled = monitor
            .get("disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if disabled {
            continue;
        }

        result.push(MonitorSpec {
            index: result.len() + 1,
            name: string_field(monitor, "name")?.to_string(),
            width: number_field(monitor, "width")? as u32,
            height: number_field(monitor, "height")? as u32,
            origin_x: number_field(monitor, "x")? as i32,
            origin_y: number_field(monitor, "y")? as i32,
        });
    }

    if result.is_empty() {
        bail!("no enabled monitors were returned by hyprctl");
    }

    Ok(result)
}

fn string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing string field `{field}`"))
}

fn number_field(value: &Value, field: &str) -> Result<i64> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("missing numeric field `{field}`"))
}
