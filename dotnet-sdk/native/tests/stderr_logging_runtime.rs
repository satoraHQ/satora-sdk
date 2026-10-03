// A test binary of its own: it sets RUST_LOG for the whole process.

#[test]
fn the_first_call_into_the_runtime_installs_logging_when_rust_log_is_set() {
    // SAFETY: the only test in this binary, so no other thread reads the environment.
    unsafe { std::env::set_var("RUST_LOG", "debug") };
    let client = satora_sdk_ffi::SatoraClient::new("http://127.0.0.1:1".into(), None).unwrap();
    assert!(!tracing::dispatcher::has_been_set());
    let _ = client.version();
    assert!(tracing::dispatcher::has_been_set());
}
