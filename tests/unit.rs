#![allow(dead_code)]

#[path = "support/mod.rs"]
mod support;

mod capture {
    include!("../src/capture.rs");

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::fs;

        include!("unit/capture_tests.rs");
    }
}

mod config {
    include!("../src/config.rs");

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::tempdir;

        include!("unit/config_tests.rs");
    }
}

mod input {
    include!("../src/input.rs");

    #[cfg(test)]
    mod tests {
        use super::*;

        include!("unit/input_tests.rs");
    }
}

mod matcher {
    mod collect {
        include!("../src/matcher/collect.rs");
    }

    mod engine {
        include!("../src/matcher/engine.rs");
    }

    mod prepare {
        include!("../src/matcher/prepare.rs");
    }

    use opencv::core::Mat;
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    pub use engine::scan_all;
    pub use prepare::prepare_rules;

    #[derive(Debug, Clone)]
    pub struct PreparedRule {
        pub target_template: String,
        pub template_path: PathBuf,
        pub template_size: (u32, u32),
        pub template_mat: Arc<Mat>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    pub struct MatchRegion {
        pub left: i32,
        pub top: i32,
        pub width: i32,
        pub height: i32,
    }

    pub type MatchSet = BTreeMap<String, Vec<MatchRegion>>;

    #[cfg(test)]
    mod tests {
        use super::collect::collect_regions;
        use super::engine::load_grayscale_mat;
        use super::prepare::prepare_rules_with_loader;
        use super::*;
        use image::{Rgba, RgbaImage};
        use opencv::core::{Mat, Scalar, CV_32FC1};
        use opencv::prelude::MatTrait;
        use std::sync::{Arc, Mutex};
        use tempfile::tempdir;

        include!("unit/matcher_tests.rs");
    }
}

mod monitor {
    include!("../src/monitor.rs");
}

mod rules {
    include!("../src/rules.rs");

    #[cfg(test)]
    mod tests {
        use super::*;

        include!("unit/rules_tests.rs");
    }
}

mod runtime {
    include!("../src/runtime.rs");

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::matcher::{MatchRegion, PreparedRule};
        use anyhow::anyhow;
        use opencv::core::{Mat, Scalar, CV_8UC1};
        use std::sync::{Arc, Mutex};

        include!("unit/runtime_tests.rs");
    }
}

mod ydotool {
    mod command {
        include!("../src/ydotool/command.rs");
    }

    mod daemon {
        include!("../src/ydotool/daemon.rs");
    }

    mod socket {
        include!("../src/ydotool/socket.rs");
    }

    use anyhow::{anyhow, bail, Context, Result};
    use std::path::PathBuf;
    use std::process::Child;
    use std::time::Duration;
    use tracing::{info, warn};

    pub(crate) use socket::{socket_accepts_connections, socket_candidates, wait_for_ready_socket};

    #[derive(Debug)]
    pub struct YdotoolManager {
        child: Option<Child>,
        owned: bool,
        socket_path: PathBuf,
    }

    impl YdotoolManager {
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

            let socket_path =
                match socket::wait_for_ready_socket(&candidates, Duration::from_secs(10)) {
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

        pub fn execute_click(&self, x: i32, y: i32) -> Result<()> {
            let socket_path = self.resolve_active_socket()?;

            let x_value = x.to_string();
            let y_value = y.to_string();
            command::run_mousemove(&socket_path, &x_value, &y_value)?;
            command::run_ydotool(&socket_path, ["click", "0xC0"])?;
            Ok(())
        }

        pub fn shutdown(&mut self) -> Result<()> {
            if let Some(child) = self.child.as_mut() {
                child.kill().context("failed to stop owned ydotoold")?;
                let _ = child.wait();
            }
            self.child = None;
            self.owned = false;
            Ok(())
        }

        pub fn owned(&self) -> bool {
            self.owned
        }

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

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::fs;
        use std::os::unix::net::UnixDatagram;
        use std::thread;
        use tempfile::tempdir;

        include!("unit/ydotool_tests.rs");
    }
}

mod app {
    mod bootstrap {
        include!("../src/app/bootstrap.rs");
    }

    mod prompts {
        include!("../src/app/prompts.rs");
    }

    mod summary {
        include!("../src/app/summary.rs");
    }

    pub(crate) use bootstrap::{prepare_runtime_rules_with, run_with_io_and_monitors};
    pub(crate) use prompts::{load_or_configure_with_io, prompt_for_config, PromptIo};
    pub(crate) use summary::render_startup_summary;

    #[cfg(test)]
    mod tests {
        use super::*;
        use anyhow::anyhow;
        use opencv::core::{Mat, Scalar, CV_8UC1};
        use std::collections::VecDeque;
        use std::fs;
        use tempfile::tempdir;

        include!("unit/app_tests.rs");
    }
}
