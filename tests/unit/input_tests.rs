#[test]
fn requests_shutdown_for_lowercase_q() {
    assert!(should_request_shutdown(b'q'));
}

#[test]
fn requests_shutdown_for_uppercase_q() {
    assert!(should_request_shutdown(b'Q'));
}

#[test]
fn ignores_other_input_bytes() {
    assert!(!should_request_shutdown(b'x'));
    assert!(!should_request_shutdown(b'1'));
    assert!(!should_request_shutdown(b'\n'));
}

/// Feeds `signals` to the shutdown handler and records the shutdown requests it
/// sent and the exit codes it asked for.
fn run_signal_handler(signals: &[i32]) -> (usize, Vec<i32>) {
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let mut exits = Vec::new();

    handle_shutdown_signals(signals.iter().copied(), &shutdown_tx, |code| exits.push(code));

    (shutdown_rx.try_iter().count(), exits)
}

#[test]
fn first_signal_requests_graceful_shutdown_without_exiting() {
    for signal in [SIGINT, SIGTERM] {
        assert_eq!(run_signal_handler(&[signal]), (1, Vec::new()));
    }
}

#[test]
fn second_signal_exits_immediately_with_status_130() {
    assert_eq!(run_signal_handler(&[SIGINT, SIGTERM]), (1, vec![130]));
}

#[test]
fn exits_once_even_when_more_signals_are_pending() {
    assert_eq!(run_signal_handler(&[SIGTERM, SIGINT, SIGINT]), (1, vec![130]));
}

#[test]
fn keeps_waiting_for_a_second_signal_after_the_shutdown_receiver_is_gone() {
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    drop(shutdown_rx);
    let mut exits = Vec::new();

    handle_shutdown_signals([SIGINT, SIGINT], &shutdown_tx, |code| exits.push(code));

    assert_eq!(exits, vec![130]);
}
