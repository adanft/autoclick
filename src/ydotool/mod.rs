mod command;
mod daemon;
mod socket;

use anyhow::{anyhow, bail, Context, Result};
use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;
use tracing::{info, warn};

/// Manages `ydotoold` lifecycle and click execution for the runtime.
#[derive(Debug)]
pub struct YdotoolManager {
    child: Option<Child>,
    owned: bool,
    socket_path: PathBuf,
}

impl YdotoolManager {
    /// Ensures `ydotool` is available and that a connectable `ydotoold` socket exists.
    ///
    /// If an external daemon is already running, it is reused. Otherwise a managed
    /// daemon is started and stopped automatically by this manager.
    pub fn ensure_ready() -> Result<Self> {
        command::verify_ydotool_binary()?;

        let candidates = socket::socket_candidates();
        let default_socket_path = candidates
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("unable to resolve ydotool socket path"))?;

        if let Some(socket_path) =
            socket::wait_for_ready_socket(&candidates, Duration::from_secs(2))
        {
            return Ok(Self {
                child: None,
                owned: false,
                socket_path,
            });
        }

        let child = daemon::spawn_managed_daemon()?;

        let daemon_pid = child.id();
        info!(
            pid = daemon_pid,
            "started managed ydotoold and waiting for a connectable socket"
        );

        let child = child;

        let socket_path = match socket::wait_for_ready_socket(&candidates, Duration::from_secs(10))
        {
            Some(path) => path,
            None => {
                let output = daemon::collect_startup_failure_output(child)
                    .context("failed while collecting ydotoold startup diagnostics")?;
                bail!(command::format_command_failure(
                    "ydotoold startup",
                    &[],
                    &default_socket_path,
                    &output,
                ));
            }
        };

        if !socket::socket_accepts_connections(&socket_path) {
            bail!("ydotoold did not become ready before timeout");
        }

        Ok(Self {
            child: Some(child),
            owned: true,
            socket_path,
        })
    }

    /// Moves the cursor through Hyprland and executes the Dota accept click through `ydotool`.
    pub fn execute_click(&self, x: i32, y: i32) -> Result<()> {
        let socket_path = self.resolve_active_socket()?;

        let x_value = x.to_string();
        let y_value = y.to_string();
        command::run_hyprctl_movecursor(&x_value, &y_value)?;
        command::run_ydotool(&socket_path, ["click", "0xC0"])?;
        Ok(())
    }

    /// Stops the managed daemon when this instance owns it.
    pub fn shutdown(&mut self) -> Result<()> {
        if let Some(child) = self.child.as_mut() {
            child.kill().context("failed to stop owned ydotoold")?;
            let _ = child.wait();
        }
        self.child = None;
        self.owned = false;
        Ok(())
    }

    /// Returns whether this manager spawned the current daemon itself.
    pub fn owned(&self) -> bool {
        self.owned
    }

    /// Resolves the socket path that should be used for the next `ydotool` command.
    fn resolve_active_socket(&self) -> Result<PathBuf> {
        if socket::socket_accepts_connections(&self.socket_path) {
            return Ok(self.socket_path.clone());
        }

        if let Some(active_socket) = socket::socket_candidates()
            .iter()
            .find(|candidate| socket::socket_accepts_connections(candidate))
        {
            warn!(resolved_socket = %self.socket_path.display(), active_socket = %active_socket.display(), "resolved ydotool socket was unavailable; using active socket");
            return Ok(active_socket.clone());
        }

        bail!(
            "ydotoold is not ready: no connectable socket found (checked: {})",
            socket::socket_candidates()
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl Drop for YdotoolManager {
    fn drop(&mut self) {
        if self.owned {
            let _ = self.shutdown();
        }
    }
}
