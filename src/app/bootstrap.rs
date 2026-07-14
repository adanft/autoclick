use super::prompts::{load_or_configure_with_io, ConsolePromptIo, PromptIo};
use super::summary::render_startup_summary;
use crate::capture::CaptureService;
use crate::config::RuleConfig;
use crate::input;
use crate::matcher::{self, PreparedRule};
use crate::monitor::{self, MonitorSpec};
use crate::runtime;
use crate::wayland_pointer::WaylandPointerBackend;
use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::sync::mpsc;
use tracing::info;

/// Starts the CLI flow: discover monitors, load configuration, and start
/// monitoring.
pub fn run() -> Result<()> {
    info!("autoclick starting");

    let monitors = monitor::enumerate_monitors()?;
    let mut io = ConsolePromptIo;
    run_with_io_and_monitors(&mut io, &monitors)
}

/// Internal entry point that allows tests to inject prompt I/O and monitor data.
pub(crate) fn run_with_io_and_monitors(
    io: &mut impl PromptIo,
    monitors: &[MonitorSpec],
) -> Result<()> {
    let store = crate::config::ConfigStore::new()?;
    let selected_config = load_or_configure_with_io(&store, monitors, io)?;

    let monitor = resolve_configured_monitor(monitors, &selected_config.monitor_name)?;

    let capture = CaptureService::new()?;
    capture
        .validate_dependency()
        .context("grim dependency check failed")?;
    let prepared_rules = prepare_runtime_rules_with(
        &selected_config.rules,
        &store.templates_dir(),
        matcher::prepare_rules,
    )?;
    let mut executor = create_wayland_backend_with(&monitor.name, WaylandPointerBackend::connect)?;

    print!(
        "{}",
        render_startup_summary(&selected_config, &prepared_rules, &monitor)
    );

    println!("monitoring started (press `q` then Enter, or send SIGINT/SIGTERM to stop)");
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let _listener =
        input::spawn_stop_listener(shutdown_tx).context("failed to install shutdown listeners")?;
    runtime::run_monitor_loop(
        &selected_config,
        &prepared_rules,
        &monitor,
        &capture,
        &mut executor,
        shutdown_rx,
    )
}

/// Resolves the configured monitor name against the current monitor list.
fn resolve_configured_monitor(monitors: &[MonitorSpec], monitor_name: &str) -> Result<MonitorSpec> {
    monitors
        .iter()
        .find(|monitor| monitor.name == monitor_name)
        .cloned()
        .ok_or_else(|| anyhow!("configured monitor `{monitor_name}` is no longer available"))
}

pub(crate) fn create_wayland_backend_with<F>(
    connector: &str,
    connect: F,
) -> Result<WaylandPointerBackend>
where
    F: FnOnce(&str) -> Result<WaylandPointerBackend>,
{
    connect(connector).context("Wayland virtual-pointer setup failed")
}

pub(crate) fn prepare_runtime_rules_with<F>(
    rules: &[RuleConfig],
    templates_dir: &Path,
    prepare_rules: F,
) -> Result<Vec<PreparedRule>>
where
    F: FnOnce(&[RuleConfig], &Path) -> Result<Vec<PreparedRule>>,
{
    prepare_rules(rules, templates_dir).context(
        "OpenCV/template validation failed; ensure OpenCV is available and template PNGs are readable",
    )
}
