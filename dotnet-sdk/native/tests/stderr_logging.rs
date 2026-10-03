// A test binary of its own: the global subscriber is per process, and any
// test that reached `runtime()` with RUST_LOG set would install one first.

#[test]
fn stderr_logging_is_installed_only_when_a_filter_is_set() {
    assert!(!satora_sdk_ffi::install_stderr_logging(None));
    assert!(!tracing::dispatcher::has_been_set());
    assert!(satora_sdk_ffi::install_stderr_logging(Some("debug".into())));
    assert!(tracing::dispatcher::has_been_set());
}
