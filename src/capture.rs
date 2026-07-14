use crate::monitor::MonitorSpec;
use crate::wayland_pointer::ImageExtent;
use anyhow::{anyhow, bail, Context, Result};
use opencv::imgcodecs;
use opencv::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// A screenshot proven to have positive dimensions by the decoder used at the capture seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedImage {
    pub path: PathBuf,
    pub extent: ImageExtent,
}

impl CapturedImage {
    pub fn from_decoded(path: PathBuf, width: i32, height: i32) -> Result<Self> {
        if width <= 0 || height <= 0 {
            bail!("captured image extent must be positive, got {width}x{height}");
        }
        Ok(Self {
            path,
            extent: ImageExtent { width, height },
        })
    }
}

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

    /// Captures and decodes a PNG screenshot for the selected monitor.
    pub fn capture_monitor(&self, monitor: &MonitorSpec) -> Result<CapturedImage> {
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

        let decoded = imgcodecs::imread(&path.to_string_lossy(), imgcodecs::IMREAD_GRAYSCALE)
            .with_context(|| format!("failed to decode captured screenshot {}", path.display()))?;
        CapturedImage::from_decoded(path, decoded.cols(), decoded.rows())
            .context("captured screenshot has no usable extent")
    }
}
