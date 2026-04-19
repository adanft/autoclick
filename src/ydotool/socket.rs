use std::env;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Polls candidate socket paths until one accepts datagram connections.
pub(crate) fn wait_for_ready_socket(candidates: &[PathBuf], timeout: Duration) -> Option<PathBuf> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| socket_accepts_connections(candidate))
        {
            return Some(candidate.clone());
        }
        thread::sleep(Duration::from_millis(100));
    }

    None
}

/// Returns `true` when a Unix datagram socket both exists and accepts connections.
pub(crate) fn socket_accepts_connections(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }

    let probe_path = env::temp_dir().join(format!(
        "autoclick-ydotool-probe-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));

    let result = UnixDatagram::bind(&probe_path)
        .and_then(|socket| socket.connect(path))
        .is_ok();

    let _ = std::fs::remove_file(&probe_path);
    result
}

/// Returns the socket discovery order used for `ydotoold` reuse or startup.
pub(crate) fn socket_candidates() -> Vec<PathBuf> {
    if let Ok(explicit) = env::var("YDOTOOL_SOCKET") {
        return vec![PathBuf::from(explicit)];
    }

    let mut candidates = vec![PathBuf::from("/tmp/.ydotool_socket")];
    if let Ok(xdg_runtime_dir) = env::var("XDG_RUNTIME_DIR") {
        candidates.push(PathBuf::from(xdg_runtime_dir).join(".ydotool_socket"));
    }
    candidates
}
