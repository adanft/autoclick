use crate::matcher::{bgr_color_stats, ColorStats};
use anyhow::{anyhow, bail, Context, Result};
use opencv::core::{self, Mat, Rect, Vec4b};
use opencv::imgproc;
use opencv::prelude::*;
use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::io::Errno;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::ErrorKind;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::time::{Duration, Instant};
use wayland_client::backend::{ReadEventsGuard, WaylandError};
use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_output::{self, WlOutput},
    wl_registry::{self, WlRegistry},
    wl_shm::{Format, WlShm},
    wl_shm_pool::WlShmPool,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

/// Bytes per pixel of every `wl_shm` format this module accepts.
const BYTES_PER_PIXEL: u32 = 4;

/// Highest `zwlr_screencopy_manager_v1` version spoken here; v3 adds `buffer_done`.
const MAX_MANAGER_VERSION: u32 = 3;

/// First `wl_output` version carrying the connector `name` event.
const OUTPUT_NAME_VERSION: u32 = 4;

/// Longest a capture waits for the compositor to finish one frame.
///
/// A healthy frame completes within a few refresh periods. One the compositor
/// never answers would otherwise block the monitor loop, and with it shutdown,
/// forever; two seconds is far beyond any normal frame yet still short enough
/// that a stalled compositor surfaces as a capture error within one cycle.
const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

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

    /// Byte offsets of blue, green and red within one pixel in memory.
    fn bgr_offsets(&self) -> Option<[usize; 3]> {
        match self.format {
            Format::Xrgb8888 | Format::Argb8888 => Some([0, 1, 2]),
            Format::Xbgr8888 | Format::Abgr8888 => Some([2, 1, 0]),
            _ => None,
        }
    }

    /// Number of bytes the frame occupies in its shm buffer.
    pub fn byte_len(&self) -> usize {
        self.stride as usize * self.height as usize
    }

    /// Checks format, extent and stride, returning the conversion code and the
    /// extent as OpenCV dimensions.
    fn validate(&self) -> Result<(i32, i32, i32)> {
        let conversion = self
            .gray_conversion()
            .with_context(|| format!("unsupported wl_shm format {:?}", self.format))?;
        let (width, height) = match (i32::try_from(self.width), i32::try_from(self.height)) {
            (Ok(width), Ok(height)) if width > 0 && height > 0 => (width, height),
            _ => bail!(
                "frame extent must be positive and fit in i32, got {}x{}",
                self.width,
                self.height
            ),
        };
        let row_bytes = u64::from(self.width) * u64::from(BYTES_PER_PIXEL);
        if u64::from(self.stride) < row_bytes || !self.stride.is_multiple_of(BYTES_PER_PIXEL) {
            bail!(
                "frame stride {} cannot hold {} pixels of {BYTES_PER_PIXEL} bytes per row",
                self.stride,
                self.width
            );
        }
        Ok((conversion, width, height))
    }

    /// Validates the layout and that `pixels` holds exactly one frame of it.
    fn validate_pixels(&self, pixels: &[u8]) -> Result<(i32, i32, i32)> {
        let validated = self.validate()?;
        if pixels.len() != self.byte_len() {
            bail!(
                "frame holds {} bytes, expected {} bytes (stride {} x height {})",
                pixels.len(),
                self.byte_len(),
                self.stride,
                self.height
            );
        }
        Ok(validated)
    }
}

/// Converts a raw `wl_shm` frame into a single-channel 8-bit grayscale matrix,
/// the same shape `imread(.., IMREAD_GRAYSCALE)` produced from a PNG capture.
///
/// Row padding past `width` is skipped and a `y_invert` frame is flipped so row
/// zero is always the top of the output.
pub fn shm_frame_to_grayscale(pixels: &[u8], layout: &FrameLayout, y_invert: bool) -> Result<Mat> {
    let (conversion, width, height) = layout.validate_pixels(pixels)?;

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

/// Mean color and luminance spread of `region` in a raw `wl_shm` frame.
///
/// `region` is in the top-down coordinates of [`shm_frame_to_grayscale`]'s
/// output: a `y_invert` frame is read bottom row first. Only the region's own
/// pixels are visited, straight from the buffer, honoring the format's channel
/// order and skipping row padding.
pub fn shm_region_stats(
    pixels: &[u8],
    layout: &FrameLayout,
    y_invert: bool,
    region: Rect,
) -> Result<ColorStats> {
    let (_, width, height) = layout.validate_pixels(pixels)?;
    let offsets = layout
        .bgr_offsets()
        .with_context(|| format!("unsupported wl_shm format {:?}", layout.format))?;
    let inside = |start: i32, length: i32, limit: i32| {
        start >= 0 && length > 0 && i64::from(start) + i64::from(length) <= i64::from(limit)
    };
    if !inside(region.x, region.width, width) || !inside(region.y, region.height, height) {
        bail!("region {region:?} lies outside the {width}x{height} frame or has no pixels");
    }

    let stride = layout.stride as usize;
    let row_start = region.x as usize * BYTES_PER_PIXEL as usize;
    let row_end = row_start + region.width as usize * BYTES_PER_PIXEL as usize;
    let region_pixels = (region.y..region.y + region.height).flat_map(|y| {
        let row = if y_invert { height - 1 - y } else { y } as usize * stride;
        pixels[row + row_start..row + row_end]
            .as_chunks::<{ BYTES_PER_PIXEL as usize }>()
            .0
            .iter()
            .map(move |pixel| offsets.map(|offset| pixel[offset]))
    });
    bgr_color_stats(region_pixels).context("color statistics need at least one pixel")
}

enum FrameOutcome {
    Ready,
    Failed,
}

/// Events of the one frame currently in flight.
#[derive(Default)]
struct FrameState {
    serial: u64,
    offer: Option<FrameLayout>,
    rejected_offers: Vec<WEnum<Format>>,
    buffers_done: bool,
    y_invert: bool,
    outcome: Option<FrameOutcome>,
}

impl FrameState {
    fn new(serial: u64) -> Self {
        Self {
            serial,
            ..Self::default()
        }
    }

    /// Whether every buffer type has been announced. Before v3 there is no
    /// `buffer_done`, so the single `buffer` event closes the offer.
    fn offer_complete(&self, has_buffer_done: bool) -> bool {
        if has_buffer_done {
            self.buffers_done
        } else {
            self.offer.is_some() || !self.rejected_offers.is_empty()
        }
    }
}

/// Callback-owned discovery and frame state. Output proxy user data is its
/// registry name.
#[derive(Default)]
struct ClientState {
    manager: Option<ZwlrScreencopyManagerV1>,
    shm: Option<WlShm>,
    outputs: BTreeMap<u32, (WlOutput, Option<String>)>,
    selected_output: Option<u32>,
    selected_output_removed: bool,
    frame: FrameState,
}

impl Dispatch<WlRegistry, ()> for ClientState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == ZwlrScreencopyManagerV1::interface().name => {
                state.manager = Some(registry.bind(name, version.min(MAX_MANAGER_VERSION), qh, ()));
            }
            wl_registry::Event::Global {
                name, interface, ..
            } if interface == WlShm::interface().name => {
                state.shm = Some(registry.bind(name, 1, qh, ()));
            }
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == WlOutput::interface().name => {
                let output = registry.bind(name, version.min(OUTPUT_NAME_VERSION), qh, name);
                state.outputs.insert(name, (output, None));
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.outputs.remove(&name);
                if state.selected_output == Some(name) {
                    state.selected_output_removed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for ClientState {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            if let Some((_, output_name)) = state.outputs.get_mut(id) {
                *output_name = Some(name);
            }
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, u64> for ClientState {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        serial: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::{Event, Flags};

        let frame = &mut state.frame;
        // A late event from an earlier, already destroyed frame must not leak
        // into the current capture.
        if frame.serial != *serial {
            return;
        }
        match event {
            Event::Buffer {
                format,
                width,
                height,
                stride,
            } => match format {
                WEnum::Value(format) if frame.offer.is_none() => {
                    let layout = FrameLayout {
                        format,
                        width,
                        height,
                        stride,
                    };
                    if layout.gray_conversion().is_some() {
                        frame.offer = Some(layout);
                    } else {
                        frame.rejected_offers.push(WEnum::Value(format));
                    }
                }
                WEnum::Unknown(_) if frame.offer.is_none() => frame.rejected_offers.push(format),
                _ => {}
            },
            Event::BufferDone => frame.buffers_done = true,
            Event::Flags {
                flags: WEnum::Value(flags),
            } => frame.y_invert = flags.contains(Flags::YInvert),
            Event::Ready { .. } => frame.outcome = Some(FrameOutcome::Ready),
            Event::Failed => frame.outcome = Some(FrameOutcome::Failed),
            // dmabuf offers and damage are irrelevant to an shm copy.
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(ClientState: ignore ZwlrScreencopyManagerV1);
wayland_client::delegate_noop!(ClientState: ignore WlShm);
wayland_client::delegate_noop!(ClientState: ignore WlShmPool);
wayland_client::delegate_noop!(ClientState: ignore WlBuffer);

/// A memfd-backed `wl_shm` buffer kept across captures of the same layout.
struct ShmBuffer {
    layout: FrameLayout,
    file: File,
    pool: WlShmPool,
    buffer: WlBuffer,
}

impl ShmBuffer {
    fn allocate(shm: &WlShm, layout: FrameLayout, qh: &QueueHandle<ClientState>) -> Result<Self> {
        let (_, width, height) = layout.validate()?;
        let size = i32::try_from(layout.byte_len()).with_context(|| {
            format!("frame of {} bytes exceeds a wl_shm pool", layout.byte_len())
        })?;
        let stride = i32::try_from(layout.stride)?;
        let fd = rustix::fs::memfd_create(c"autoclick-screencopy", rustix::fs::MemfdFlags::CLOEXEC)
            .context("failed to create a memfd for the screencopy buffer")?;
        let file = File::from(fd);
        file.set_len(layout.byte_len() as u64)
            .context("failed to size the screencopy memfd")?;
        let pool = shm.create_pool(file.as_fd(), size, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, layout.format, qh, ());
        Ok(Self {
            layout,
            file,
            pool,
            buffer,
        })
    }

    fn destroy(self) {
        self.buffer.destroy();
        self.pool.destroy();
    }
}

/// Captures one Wayland output in-process through `zwlr_screencopy_manager_v1`.
///
/// Owns a persistent connection and reuses its shm buffer while the frame
/// layout stays the same, so a steady-state capture allocates nothing new.
pub struct ScreencopyClient {
    connection: Connection,
    event_queue: EventQueue<ClientState>,
    state: ClientState,
    manager: ZwlrScreencopyManagerV1,
    output: WlOutput,
    connector: String,
    buffer: Option<ShmBuffer>,
    pixels: Vec<u8>,
    /// Layout and `y_invert` of the frame held in `pixels`, while it is a
    /// complete capture; cleared when a new capture starts.
    captured: Option<(FrameLayout, bool)>,
    next_serial: u64,
    frame_timeout: Duration,
}

impl ScreencopyClient {
    /// Connects to the compositor from the environment and selects the output
    /// named `connector`. Fails when screencopy, `wl_shm` or the output is missing.
    pub fn connect(connector: &str) -> Result<Self> {
        let connection =
            Connection::connect_to_env().context("failed to connect to the Wayland compositor")?;
        Self::from_connection(connection, connector)
    }

    fn from_connection(connection: Connection, connector: &str) -> Result<Self> {
        let mut event_queue = connection.new_event_queue();
        // Not retained: `wl_registry` has no destroy request.
        connection.display().get_registry(&event_queue.handle(), ());
        let mut state = ClientState::default();
        event_queue
            .roundtrip(&mut state)
            .context("Wayland registry discovery roundtrip failed")?;
        // Outputs bound in the first roundtrip announce their names in the second.
        event_queue
            .roundtrip(&mut state)
            .context("Wayland output metadata discovery roundtrip failed")?;

        let manager = state.manager.clone().ok_or_else(|| {
            anyhow!("the compositor does not advertise zwlr_screencopy_manager_v1 (wlr-screencopy)")
        })?;
        if state.shm.is_none() {
            bail!("the compositor does not advertise wl_shm");
        }
        let matching: Vec<_> = state
            .outputs
            .iter()
            .filter(|(_, (_, name))| name.as_deref() == Some(connector))
            .map(|(&id, (output, _))| (id, output.clone()))
            .collect();
        let (id, output) = match matching.as_slice() {
            [selected] => selected.clone(),
            [] => bail!("configured connector {connector} was not found among Wayland outputs"),
            matches => bail!(
                "configured connector {connector} is ambiguous: {} Wayland outputs match",
                matches.len()
            ),
        };
        state.selected_output = Some(id);

        Ok(Self {
            connection,
            event_queue,
            state,
            manager,
            output,
            connector: connector.into(),
            buffer: None,
            pixels: Vec::new(),
            captured: None,
            next_serial: 0,
            frame_timeout: FRAME_TIMEOUT,
        })
    }

    /// Captures the selected output, without the cursor, as a grayscale matrix.
    ///
    /// A failed frame, or one not finished within [`FRAME_TIMEOUT`], is destroyed
    /// and reported; the next call starts a fresh one.
    pub fn capture(&mut self) -> Result<Mat> {
        if self.state.selected_output_removed {
            bail!("Wayland output {} was removed", self.connector);
        }
        self.captured = None;
        let deadline = Instant::now() + self.frame_timeout;
        self.next_serial += 1;
        self.state.frame = FrameState::new(self.next_serial);
        let frame = self.manager.capture_output(
            0,
            &self.output,
            &self.event_queue.handle(),
            self.next_serial,
        );
        let captured = self.copy_frame(&frame, deadline);
        frame.destroy();
        let flushed = self
            .connection
            .flush()
            .context("failed to flush the screencopy frame destruction");
        let image = captured?;
        flushed?;
        Ok(image)
    }

    fn copy_frame(&mut self, frame: &ZwlrScreencopyFrameV1, deadline: Instant) -> Result<Mat> {
        let has_buffer_done = frame.version() >= 3;
        self.dispatch_until(deadline, |frame| {
            frame.outcome.is_some() || frame.offer_complete(has_buffer_done)
        })?;
        if self.state.frame.outcome.is_some() {
            bail!("the compositor failed the screencopy frame before offering a buffer");
        }
        let layout = self.state.frame.offer.ok_or_else(|| {
            anyhow!(
                "the compositor offered no supported wl_shm screencopy format: {:?}",
                self.state.frame.rejected_offers
            )
        })?;

        frame.copy(&self.reusable_buffer(layout)?.buffer);
        self.dispatch_until(deadline, |frame| frame.outcome.is_some())?;
        if let Some(FrameOutcome::Failed) = self.state.frame.outcome {
            bail!("the compositor reported the screencopy frame as failed");
        }

        let buffer = self
            .buffer
            .as_ref()
            .context("screencopy shm buffer disappeared during the copy")?;
        self.pixels.resize(layout.byte_len(), 0);
        buffer
            .file
            .read_exact_at(&mut self.pixels, 0)
            .context("failed to read the screencopy shm buffer")?;
        let y_invert = self.state.frame.y_invert;
        let gray = shm_frame_to_grayscale(&self.pixels, &layout, y_invert)?;
        self.captured = Some((layout, y_invert));
        Ok(gray)
    }

    /// Mean color and luminance spread of `region` in the last captured frame,
    /// in the coordinates of the grayscale matrix [`Self::capture`] returned.
    ///
    /// Reads the region straight from the frame's pixels, which stay in memory
    /// until the next capture, so no color copy of the frame is ever made.
    pub fn region_stats(&self, region: Rect) -> Result<ColorStats> {
        let (layout, y_invert) = self
            .captured
            .context("no captured frame to sample: the last capture failed or none ran yet")?;
        shm_region_stats(&self.pixels, &layout, y_invert, region)
    }

    /// Returns the cached buffer when the layout is unchanged, else replaces it.
    fn reusable_buffer(&mut self, layout: FrameLayout) -> Result<&ShmBuffer> {
        if self.buffer.as_ref().map(|buffer| buffer.layout) != Some(layout) {
            if let Some(stale) = self.buffer.take() {
                stale.destroy();
            }
            let shm = self.state.shm.as_ref().context("wl_shm is unavailable")?;
            self.buffer = Some(ShmBuffer::allocate(
                shm,
                layout,
                &self.event_queue.handle(),
            )?);
        }
        self.buffer
            .as_ref()
            .context("screencopy shm buffer is unavailable")
    }

    /// Dispatches frame events until `done` holds or `deadline` passes.
    ///
    /// `blocking_dispatch` would wait on the socket with no bound, so this
    /// polls the connection fd with the time left instead.
    fn dispatch_until(
        &mut self,
        deadline: Instant,
        done: impl Fn(&FrameState) -> bool,
    ) -> Result<()> {
        loop {
            self.event_queue
                .dispatch_pending(&mut self.state)
                .context("Wayland dispatch failed while waiting for a screencopy frame")?;
            if done(&self.state.frame) {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!(
                    "screencopy frame for output {} did not complete within {} ms",
                    self.connector,
                    self.frame_timeout.as_millis()
                );
            }
            self.event_queue
                .flush()
                .context("failed to flush screencopy requests")?;
            // `None` means events are already queued: dispatch them first.
            if let Some(guard) = self.event_queue.prepare_read() {
                if wait_readable(&guard, remaining)? {
                    match guard.read() {
                        Ok(_) => {}
                        Err(WaylandError::Io(error)) if error.kind() == ErrorKind::WouldBlock => {}
                        Err(error) => {
                            return Err(error)
                                .context("failed to read Wayland events for a screencopy frame")
                        }
                    }
                }
            }
        }
    }
}

/// Waits until the connection fd is readable or `timeout` elapses, returning
/// whether it became readable. An interrupted wait reports not readable so the
/// caller rechecks its deadline.
fn wait_readable(guard: &ReadEventsGuard, timeout: Duration) -> Result<bool> {
    let fd = guard.connection_fd();
    let mut fds = [PollFd::new(&fd, PollFlags::IN | PollFlags::ERR)];
    let timeout = Timespec::try_from(timeout).context("screencopy timeout out of range")?;
    match rustix::event::poll(&mut fds, Some(&timeout)) {
        Ok(ready) => Ok(ready > 0),
        Err(Errno::INTR) => Ok(false),
        Err(error) => Err(error).context("failed to poll the Wayland connection"),
    }
}

impl Drop for ScreencopyClient {
    /// Releases the shm buffer and the manager on shutdown.
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            buffer.destroy();
        }
        self.manager.destroy();
        if let Err(error) = self.connection.flush() {
            tracing::warn!(error = %error, "Wayland screencopy cleanup failed during shutdown");
        }
    }
}
