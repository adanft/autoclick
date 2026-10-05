use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

pub fn capture_env(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

pub fn restore_env(key: &str, value: Option<OsString>) {
    match value {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

/// Runs `body` and returns everything it logged from this thread.
///
/// The subscriber is global and installed exactly once. A thread-local
/// subscriber is not enough: `tracing` caches callsite interest process-wide, so
/// a test logging in parallel while no subscriber is installed can pin the very
/// event under test to "never" and drop it. A global subscriber that always
/// accepts DEBUG leaves no window for that.
pub fn capture_debug_logs(body: impl FnOnce()) -> String {
    install_capturing_subscriber();

    let buffer = LogBuffer::default();
    SINK.with(|sink| *sink.borrow_mut() = Some(buffer.clone()));
    body();
    SINK.with(|sink| *sink.borrow_mut() = None);

    let logged = buffer.0.lock().unwrap_or_else(|error| error.into_inner());
    String::from_utf8(logged.clone()).expect("subscriber wrote invalid UTF-8")
}

fn install_capturing_subscriber() {
    static INSTALLED: OnceLock<()> = OnceLock::new();

    INSTALLED.get_or_init(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(ThreadSink)
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .without_time()
            .finish();
        // Another test binary target may have installed one already; the sink is
        // per thread either way, so a failure here is not this helper's problem.
        let _ = tracing::subscriber::set_global_default(subscriber);
        // Re-evaluate callsites that were already visited and cached before this
        // subscriber existed.
        tracing::callsite::rebuild_interest_cache();
    });
}

thread_local! {
    /// Where this thread's log lines go, when it is capturing them.
    static SINK: std::cell::RefCell<Option<LogBuffer>> = const { std::cell::RefCell::new(None) };
}

/// Routes every thread's log lines to that thread's buffer, discarding the rest.
struct ThreadSink;

impl std::io::Write for ThreadSink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        SINK.with(|sink| {
            if let Some(target) = sink.borrow().as_ref() {
                target
                    .0
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .extend_from_slice(buffer);
            }
        });
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for ThreadSink {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        Self
    }
}

#[derive(Clone, Default)]
struct LogBuffer(std::sync::Arc<Mutex<Vec<u8>>>);

/// One pixel in OpenCV's blue, green, red channel order.
pub type Bgr = [u8; 3];

/// A 24x12 BGR button: a `fill` face carrying two `text` bars, one of them
/// taller, so grayscale matching locates it in both axes.
pub fn button(fill: Bgr, text: Bgr) -> opencv::core::Mat {
    bgr_image(24, 12, |row, col| {
        let short_bar = (4..8).contains(&row) && (3..9).contains(&col);
        let tall_bar = (2..10).contains(&row) && (13..20).contains(&col);
        if short_bar || tall_bar {
            text
        } else {
            fill
        }
    })
}

/// A `width` x `height` BGR image whose pixel at (`row`, `col`) is `pixel(row, col)`.
pub fn bgr_image(width: i32, height: i32, pixel: impl Fn(i32, i32) -> Bgr) -> opencv::core::Mat {
    use opencv::core::{Mat, Scalar, Vec3b, CV_8UC3};
    use opencv::prelude::*;

    let mut image = Mat::new_rows_cols_with_default(height, width, CV_8UC3, Scalar::all(0.0))
        .expect("failed to allocate a BGR test image");
    for row in 0..height {
        for col in 0..width {
            *image.at_2d_mut::<Vec3b>(row, col).unwrap() = Vec3b::from(pixel(row, col));
        }
    }
    image
}

/// Maps every pixel of a BGR image through `map`.
pub fn map_pixels(image: &opencv::core::Mat, map: impl Fn(Bgr) -> Bgr) -> opencv::core::Mat {
    use opencv::core::Vec3b;
    use opencv::prelude::MatTraitConst;

    bgr_image(image.cols(), image.rows(), |row, col| {
        map(image.at_2d::<Vec3b>(row, col).unwrap().0)
    })
}

/// A `background` BGR screen with `stamp` copied at (`left`, `top`).
pub fn screen_with(
    width: i32,
    height: i32,
    background: Bgr,
    stamp: &opencv::core::Mat,
    left: i32,
    top: i32,
) -> opencv::core::Mat {
    use opencv::core::Vec3b;
    use opencv::prelude::MatTraitConst;

    bgr_image(width, height, |row, col| {
        let inside =
            (left..left + stamp.cols()).contains(&col) && (top..top + stamp.rows()).contains(&row);
        if inside {
            stamp.at_2d::<Vec3b>(row - top, col - left).unwrap().0
        } else {
            background
        }
    })
}

/// The grayscale matrix the matcher would see for a BGR image.
pub fn to_gray(image: &opencv::core::Mat) -> opencv::core::Mat {
    let mut gray = opencv::core::Mat::default();
    opencv::imgproc::cvt_color_def(image, &mut gray, opencv::imgproc::COLOR_BGR2GRAY)
        .expect("failed to convert a BGR test image to grayscale");
    gray
}

/// The face and text colors of the synthetic button the color-check tests target.
pub const BUTTON_FILL: Bgr = [60, 170, 40];
pub const BUTTON_TEXT: Bgr = [255, 255, 255];
/// The dark blue screen background around the synthetic button.
pub const SCREEN_BACKGROUND: Bgr = [90, 40, 30];
/// A red face with the same BT.601 luma as [`BUTTON_FILL`] (both 119 in gray).
pub const SAME_LUMA_RED_FILL: Bgr = [80, 70, 230];

/// Blends every channel `fraction` of the way from `pixel` toward `target`.
pub fn blend_toward(pixel: Bgr, target: u8, fraction: f64) -> Bgr {
    pixel.map(|channel| {
        let channel = f64::from(channel);
        (channel + (f64::from(target) - channel) * fraction).round() as u8
    })
}

/// Replaces every channel with the pixel's BT.601 luma, keeping its brightness.
pub fn desaturate(pixel: Bgr) -> Bgr {
    let [blue, green, red] = pixel.map(f64::from);
    let luma = (0.114 * blue + 0.587 * green + 0.299 * red).round() as u8;
    [luma; 3]
}
