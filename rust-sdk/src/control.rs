//! Cooperative cancellation and bounded polling shared by long-running operations.
use crate::Error;
use crate::Result;
use std::future::Future;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Cancellation(tokio::sync::watch::Sender<bool>);
impl Default for Cancellation {
    fn default() -> Self {
        Self(tokio::sync::watch::channel(false).0)
    }
}
impl Cancellation {
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }
    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
    pub async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|cancelled| *cancelled).await;
    }
}

#[derive(Clone, Debug)]
pub struct PollOptions {
    pub interval: Duration,
    /// Includes HTTP requests, RPC calls, and delays.
    pub timeout: Duration,
    pub cancellation: Cancellation,
}
impl Default for PollOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(5),
            timeout: Duration::from_secs(300),
            cancellation: Cancellation::default(),
        }
    }
}
impl PollOptions {
    pub async fn run<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
        if self.interval.is_zero() || self.timeout.is_zero() {
            return Err(Error::InvalidSwap(
                "poll interval and timeout must be positive".into(),
            ));
        }
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(Error::Cancelled),
            result = tokio::time::timeout(self.timeout, future) => {
                result.map_err(|_| Error::Timeout("operation deadline exceeded".into()))?
            }
        }
    }
}
