use anyhow::{Context, Result};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::io::{self, Read};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

#[derive(Debug)]
pub struct ShutdownListeners {
    _stdin_listener: JoinHandle<()>,
    _signal_listener: JoinHandle<()>,
}

/// Spawns background listeners that request shutdown on `q`, `SIGINT`, or `SIGTERM`.
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
        if signals.forever().next().is_some() {
            let _ = shutdown_tx.send(());
        }
    });

    Ok(ShutdownListeners {
        _stdin_listener: stdin_listener,
        _signal_listener: signal_listener,
    })
}

fn should_request_shutdown(byte: u8) -> bool {
    byte == b'q' || byte == b'Q'
}
