use anyhow::{Context, Result};
use std::process::{Child, Command, Output, Stdio};

/// Starts a managed `ydotoold` instance whose lifecycle is tied to the manager.
pub(crate) fn spawn_managed_daemon() -> Result<Child> {
    Command::new("ydotoold")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start ydotoold")
}

/// Stops a child process after startup timeout and captures its diagnostics.
pub(crate) fn collect_startup_failure_output(mut child: Child) -> Result<Output> {
    if child.try_wait()?.is_none() {
        child
            .kill()
            .context("failed to stop ydotoold after startup timeout")?;
    }

    child
        .wait_with_output()
        .context("failed to collect ydotoold startup output")
}
