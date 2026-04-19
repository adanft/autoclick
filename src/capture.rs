use crate::monitor::MonitorSpec;
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// Captures monitor screenshots into a temporary working directory.
pub struct CaptureService {
    temp_dir: TempDir,
}

impl CaptureService {
    /// Creates a capture service backed by a fresh temporary directory.
    pub fn new() -> Result<Self> {
        Ok(Self {
            temp_dir: tempfile::tempdir().context("failed to create screenshot temp directory")?,
        })
    }

    /// Verifies that the `grim` dependency is installed and executable.
    pub fn validate_dependency(&self) -> Result<()> {
        let output = Command::new("grim")
            .arg("-h")
            .output()
            .context("failed to execute grim")?;

        if !output.status.success() {
            bail!("grim is unavailable or returned a non-zero status");
        }

        Ok(())
    }

    /// Captures a PNG screenshot for the selected monitor and returns the file path.
    pub fn capture_monitor(&self, monitor: &MonitorSpec) -> Result<PathBuf> {
        let path = self.temp_dir.path().join("capture.png");

        let output = Command::new("grim")
            .arg("-o")
            .arg(&monitor.name)
            .arg(&path)
            .output()
            .with_context(|| format!("failed to execute grim for monitor {}", monitor.name))?;

        if !output.status.success() {
            bail!(
                "grim failed for monitor {}: {}",
                monitor.name,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        if !Path::new(&path).exists() {
            return Err(anyhow!(
                "grim completed without producing a screenshot file"
            ));
        }

        Ok(path)
    }
}
