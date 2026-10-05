use anyhow::{bail, Context, Result};
use opencv::core::{self, Mat, Rect, Vec4b};
use opencv::imgproc;
use opencv::prelude::*;
use wayland_client::protocol::wl_shm::Format;

/// Bytes per pixel of every `wl_shm` format this module accepts.
const BYTES_PER_PIXEL: u32 = 4;

/// Pixel layout of a `wl_shm` frame as announced by the screencopy `buffer` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLayout {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

impl FrameLayout {
    /// OpenCV conversion code reading this format's little-endian memory order.
    fn gray_conversion(&self) -> Option<i32> {
        match self.format {
            // 0xXXRRGGBB words: B, G, R, X in memory.
            Format::Xrgb8888 | Format::Argb8888 => Some(imgproc::COLOR_BGRA2GRAY),
            // 0xXXBBGGRR words: R, G, B, X in memory.
            Format::Xbgr8888 | Format::Abgr8888 => Some(imgproc::COLOR_RGBA2GRAY),
            _ => None,
        }
    }

    /// Number of bytes the frame occupies in its shm buffer.
    pub fn byte_len(&self) -> usize {
        self.stride as usize * self.height as usize
    }
}

/// Converts a raw `wl_shm` frame into a single-channel 8-bit grayscale matrix,
/// the same shape `imread(.., IMREAD_GRAYSCALE)` produced from a PNG capture.
///
/// Row padding past `width` is skipped and a `y_invert` frame is flipped so row
/// zero is always the top of the output.
pub fn shm_frame_to_grayscale(pixels: &[u8], layout: &FrameLayout, y_invert: bool) -> Result<Mat> {
    let conversion = layout
        .gray_conversion()
        .with_context(|| format!("unsupported wl_shm format {:?}", layout.format))?;
    let (width, height) = match (i32::try_from(layout.width), i32::try_from(layout.height)) {
        (Ok(width), Ok(height)) if width > 0 && height > 0 => (width, height),
        _ => bail!(
            "frame extent must be positive and fit in i32, got {}x{}",
            layout.width,
            layout.height
        ),
    };
    let row_bytes = u64::from(layout.width) * u64::from(BYTES_PER_PIXEL);
    if u64::from(layout.stride) < row_bytes || !layout.stride.is_multiple_of(BYTES_PER_PIXEL) {
        bail!(
            "frame stride {} cannot hold {} pixels of {BYTES_PER_PIXEL} bytes per row",
            layout.stride,
            layout.width
        );
    }
    if pixels.len() != layout.byte_len() {
        bail!(
            "frame holds {} bytes, expected {} bytes (stride {} x height {})",
            pixels.len(),
            layout.byte_len(),
            layout.stride,
            layout.height
        );
    }

    // View the buffer with the padding as extra columns, then crop them away;
    // `cvt_color` reads the non-continuous ROI without copying it first.
    let padded_cols = i32::try_from(layout.stride / BYTES_PER_PIXEL)?;
    let padded = Mat::new_rows_cols_with_bytes::<Vec4b>(height, padded_cols, pixels)
        .context("failed to view the shm frame as a 4-channel matrix")?;
    let visible = padded
        .roi(Rect::new(0, 0, width, height))
        .context("failed to crop the shm frame row padding")?;

    let mut gray = Mat::default();
    imgproc::cvt_color_def(&visible, &mut gray, conversion)
        .context("failed to convert the shm frame to grayscale")?;
    if !y_invert {
        return Ok(gray);
    }
    let mut flipped = Mat::default();
    core::flip(&gray, &mut flipped, 0).context("failed to flip a y-inverted shm frame")?;
    Ok(flipped)
}
