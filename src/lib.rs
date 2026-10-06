//! Core library for the `autoclick` binary.
//!
//! The runtime flow is intentionally split into small modules: discover
//! monitors, capture a screenshot, run OpenCV template matching, evaluate the
//! configured rules, and dispatches clicks through an output-bound Wayland virtual pointer.

use tracing_subscriber::fmt::{format, MakeWriter, SubscriberBuilder};
use tracing_subscriber::EnvFilter;

pub mod app;
pub mod capture;
pub mod config;
pub mod input;
pub mod matcher;
pub mod monitor;
pub mod rules;
pub mod runtime;
pub mod screencopy;
pub mod wayland_pointer;

/// Initializes stderr logging with `RUST_LOG`, defaulting to warnings and errors
/// so a skipped monitor cycle is visible without extra configuration.
pub fn init_logging() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    let _ = log_format(std::io::stderr)
        .with_env_filter(env_filter)
        .try_init();
}

/// The log line format, writing to `writer`: every line starts with its UTC
/// timestamp, so an unattended run shows when an output went away and came
/// back, and leaves out the module path.
pub fn log_format<W>(
    writer: W,
) -> SubscriberBuilder<format::DefaultFields, format::Format, tracing::level_filters::LevelFilter, W>
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_target(false)
}
