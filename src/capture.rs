use crate::screencopy::ScreencopyClient;
use crate::wayland_pointer::ImageExtent;
use anyhow::{bail, Context, Result};
use opencv::core::Mat;
use opencv::prelude::*;

/// A screenshot proven to have positive dimensions at the capture seam.
///
/// The grayscale matrix travels with the capture so the matcher works on it
/// directly instead of decoding the screen a second time.
#[derive(Debug)]
pub struct CapturedImage {
    pub extent: ImageExtent,
    pub image: Mat,
}

impl CapturedImage {
    pub fn from_decoded(image: Mat) -> Result<Self> {
        let (width, height) = (image.cols(), image.rows());
        if width <= 0 || height <= 0 {
            bail!("captured image extent must be positive, got {width}x{height}");
        }
        Ok(Self {
            extent: ImageExtent { width, height },
            image,
        })
    }
}

/// Produces grayscale frames of one output.
///
/// The seam between the capture service and the compositor, so the service can
/// be exercised without a Wayland session.
pub trait FrameSource {
    fn capture_frame(&mut self) -> Result<Mat>;
}

impl FrameSource for ScreencopyClient {
    fn capture_frame(&mut self) -> Result<Mat> {
        self.capture()
    }
}

/// Captures one configured output through a persistent frame source.
pub struct CaptureService<S = ScreencopyClient> {
    connector: String,
    source: S,
}

impl CaptureService {
    /// Connects to the compositor's wlr-screencopy manager for `connector`.
    pub fn connect(connector: &str) -> Result<Self> {
        Ok(Self::with_source(
            connector,
            ScreencopyClient::connect(connector)?,
        ))
    }
}

impl<S: FrameSource> CaptureService<S> {
    /// Wraps an already-connected frame source bound to `connector`.
    pub fn with_source(connector: &str, source: S) -> Self {
        Self {
            connector: connector.to_string(),
            source,
        }
    }

    /// Captures the bound output as a grayscale screenshot.
    pub fn capture_monitor(&mut self) -> Result<CapturedImage> {
        let image = self
            .source
            .capture_frame()
            .with_context(|| format!("failed to capture output {}", self.connector))?;
        CapturedImage::from_decoded(image).context("captured screenshot has no usable extent")
    }
}
