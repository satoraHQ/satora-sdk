//! Clock and timer primitives for the native Tokio and JS host runtimes.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) use std::time::Instant as Monotonic;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) use std::time::SystemTime;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) use std::time::UNIX_EPOCH;
#[cfg(all(
    feature = "gasless",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub(crate) use tokio::time::Instant;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) use tokio::time::sleep;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) use tokio::time::timeout;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) use wasm::sleep;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) use wasm::timeout;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) use web_time::Instant as Monotonic;
#[cfg(all(feature = "gasless", target_arch = "wasm32", target_os = "unknown"))]
pub(crate) use web_time::Instant;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) use web_time::SystemTime;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) use web_time::UNIX_EPOCH;

/// Bounds the Fetch abort timer to the range accepted by JavaScript.
#[cfg(feature = "gasless")]
pub(crate) fn http_timeout(duration: std::time::Duration) -> std::time::Duration {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    let duration = duration.min(std::time::Duration::from_millis(i32::MAX as u64));
    duration
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod wasm {
    use gloo_timers::future::TimeoutFuture;
    use std::future::Future;
    use std::time::Duration;

    pub(crate) async fn sleep(duration: Duration) {
        // JS timers take a signed 32-bit count of milliseconds. Split longer
        // waits and round fractional milliseconds up to avoid early expiry.
        const MAX_MS: u128 = i32::MAX as u128;
        let mut millis = duration.as_nanos().div_ceil(1_000_000);
        while millis > MAX_MS {
            TimeoutFuture::new(MAX_MS as u32).await;
            millis -= MAX_MS;
        }
        TimeoutFuture::new(millis as u32).await;
    }

    pub(crate) async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, ()> {
        tokio::select! {
            biased;
            value = future => Ok(value),
            () = sleep(duration) => Err(()),
        }
    }
}
