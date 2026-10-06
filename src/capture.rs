use crate::matcher::ColorStats;
use crate::screencopy::ScreencopyClient;
use crate::wayland_pointer::ImageExtent;
use anyhow::{bail, Context, Result};
use opencv::core::{Mat, Rect};
use opencv::prelude::*;
use std::fmt;

/// A capture or click failure caused by losing the output or the compositor
/// link behind it, rather than by one bad frame or click.
///
/// Neither Wayland client can recover from it in place: a returning output is
/// a new `wl_output` global and a failed connection stays failed. It travels
/// in the `anyhow` chain of the failure so the runtime can recognize it with
/// [`Disconnect::find`] and rebuild both clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disconnect {
    /// The compositor withdrew the selected output's `wl_output` global.
    OutputRemoved { connector: String },
    /// The Wayland connection failed and cannot be used again.
    ConnectionLost,
    /// The compositor did not answer a round trip within its deadline, so
    /// the connection can no longer be trusted to deliver anything.
    Unresponsive,
}

impl Disconnect {
    /// The disconnect anywhere in `error`'s context chain, if it has one.
    pub fn find(error: &anyhow::Error) -> Option<&Self> {
        error.downcast_ref()
    }
}

impl fmt::Display for Disconnect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutputRemoved { connector } => {
                write!(f, "Wayland output {connector} was removed")
            }
            Self::ConnectionLost => f.write_str("the Wayland connection was lost"),
            Self::Unresponsive => f.write_str("the Wayland compositor stopped answering"),
        }
    }
}

impl std::error::Error for Disconnect {}

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

/// Produces grayscale frames of one output, and samples the color of the last.
///
/// The seam between the capture service and the compositor, so the service can
/// be exercised without a Wayland session.
pub trait FrameSource {
    fn capture_frame(&mut self) -> Result<Mat>;

    /// Mean color and luminance spread of `region` of the last captured frame,
    /// in the coordinates of its grayscale matrix. Fails when the last capture
    /// failed or none ran yet.
    fn region_stats(&self, region: Rect) -> Result<ColorStats>;
}

impl FrameSource for ScreencopyClient {
    fn capture_frame(&mut self) -> Result<Mat> {
        self.capture()
    }

    fn region_stats(&self, region: Rect) -> Result<ColorStats> {
        ScreencopyClient::region_stats(self, region)
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

    /// Samples the colors of `region` in the last captured screenshot.
    pub fn region_stats(&self, region: Rect) -> Result<ColorStats> {
        self.source
            .region_stats(region)
            .with_context(|| format!("failed to sample output {}", self.connector))
    }
}
