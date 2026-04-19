//! Core library for the `autoclick` binary.
//!
//! The runtime flow is intentionally split into small modules: discover
//! monitors, capture a screenshot, run OpenCV template matching, evaluate the
//! configured rules, and dispatch clicks through `ydotool`.

use tracing_subscriber::EnvFilter;

pub mod app;
pub mod capture;
pub mod config;
pub mod input;
pub mod matcher;
pub mod monitor;
pub mod rules;
pub mod runtime;
pub mod ydotool;

/// Initializes stderr logging with `RUST_LOG`, defaulting to errors only.
pub fn init_logging() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .try_init();
}
