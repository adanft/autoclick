use anyhow::{Context, Result};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::io::{self, Read};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

/// Exit status of a forced shutdown: the shell convention `128 + SIGINT`.
const FORCED_EXIT_CODE: i32 = 130;

#[derive(Debug)]
pub struct ShutdownListeners {
    _stdin_listener: JoinHandle<()>,
    _signal_listener: JoinHandle<()>,
}

/// Spawns background listeners that request shutdown on `q`, `SIGINT`, or `SIGTERM`.
///
/// The signal listener lives for the whole process: a second `SIGINT` or
/// `SIGTERM` exits immediately, so a cycle stuck in the compositor never makes
/// the process unkillable.
pub fn spawn_stop_listener(shutdown_tx: Sender<()>) -> Result<ShutdownListeners> {
    let stdin_tx = shutdown_tx.clone();
    let stdin_listener = thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buffer = [0_u8; 1];

        loop {
            match stdin.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) if should_request_shutdown(buffer[0]) => {
                    let _ = stdin_tx.send(());
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });

    let mut signals = Signals::new([SIGINT, SIGTERM])
        .context("failed to register SIGINT/SIGTERM shutdown handlers")?;
    let signal_listener = thread::spawn(move || {
        handle_shutdown_signals(signals.forever(), &shutdown_tx, |code| {
            std::process::exit(code)
        });
    });

    Ok(ShutdownListeners {
        _stdin_listener: stdin_listener,
        _signal_listener: signal_listener,
    })
}

/// Requests a graceful shutdown on the first signal and calls `exit` with
/// [`FORCED_EXIT_CODE`] on the second, without waiting for the monitor loop.
///
/// Keeping the iterator alive keeps the handlers registered: dropping
/// `Signals` unregisters them without restoring the default action, which would
/// leave every later signal ignored.
fn handle_shutdown_signals(
    signals: impl IntoIterator<Item = i32>,
    shutdown_tx: &Sender<()>,
    exit: impl FnOnce(i32),
) {
    let mut signals = signals.into_iter();
    if signals.next().is_none() {
        return;
    }
    // The receiver is gone once the loop has stopped; the process is exiting anyway.
    let _ = shutdown_tx.send(());
    if signals.next().is_some() {
        eprintln!("second shutdown signal received, exiting immediately");
        exit(FORCED_EXIT_CODE);
    }
}

fn should_request_shutdown(byte: u8) -> bool {
    byte == b'q' || byte == b'Q'
}
