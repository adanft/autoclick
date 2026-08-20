use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
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

pub fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).expect("failed to write script");
    let mut permissions = fs::metadata(path)
        .expect("failed to stat script")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("failed to chmod script");
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
