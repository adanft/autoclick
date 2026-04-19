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
