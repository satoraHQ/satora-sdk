//! Wire a dedicated contract manager to persistent tracking. An application can
//! supply an executor like BitcoinClaimer for explicitly approved claim policy.
use lendaswap_sdk::Client;
use lendaswap_sdk::Error;
use lendaswap_sdk::Result;
use lendaswap_sdk::actions::ActionKind;
use lendaswap_sdk::bitcoin::BitcoinBackend;
use lendaswap_sdk::bitcoin::BitcoinSpendOptions;
use lendaswap_sdk::bitcoin::EsploraBackend;
use lendaswap_sdk::observation::BitcoinReader;
use lendaswap_sdk::observation::ContractManager;
use lendaswap_sdk::observation::ContractReader;
use lendaswap_sdk::observation::ManagerOptions;
use lendaswap_sdk::observation::ReferenceOptions;
use lendaswap_sdk::tracker::ActionExecutor;
use lendaswap_sdk::tracker::ExecutionFuture;
use lendaswap_sdk::tracker::ExecutionResult;
use lendaswap_sdk::tracker::SwapTracker;
use lendaswap_sdk::tracker::TrackerOptions;
use lendaswap_sdk::tracker::TrackerWorker;
use lendaswap_sdk::tracker::TrackingOptions;
use std::sync::Arc;

pub fn bitcoin_reader(url: &str, network: bitcoin::Network) -> Result<Arc<dyn ContractReader>> {
    Ok(Arc::new(BitcoinReader::new(Arc::new(EsploraBackend::new(
        url, network,
    )?))))
}
/// For multi-ledger swaps pass a ReaderSet containing every required reader.
/// Configure EVM scan start, coordinator and locked token in `references`.
pub async fn track_stored(
    client: Client,
    reader: Arc<dyn ContractReader>,
    id: &str,
    references: ReferenceOptions,
) -> Result<(SwapTracker, TrackerWorker)> {
    let contracts = client.contract_references(id, &references)?;
    let manager = ContractManager::new(reader, ManagerOptions::default())?;
    // A retired swap keeps its journal; `resume` it rather than tracking it again.
    let journaled = client
        .get_stored_swap(id)?
        .is_some_and(|swap| swap.metadata.tracking.is_some());
    let tracker = SwapTracker::new(client, manager, TrackerOptions::default(), None)?;
    let failures = tracker.restore().await?;
    if !failures.is_empty() {
        return Err(Error::Storage(format!("restore failures: {failures:?}")));
    }
    if !journaled {
        tracker
            .track(id, contracts, TrackingOptions::default())
            .await?;
    } else if tracker.record(id).await.is_err() {
        tracker.resume(id).await?;
    }
    let worker = tracker.start()?;
    Ok((tracker, worker))
}
/// Register this adapter only for EVM→Bitcoin swaps. The destination/fee limits
/// are application-approved; a transport error is an uncertain submission.
pub struct BitcoinClaimer {
    pub client: Client,
    pub backend: Arc<dyn BitcoinBackend>,
    pub options: BitcoinSpendOptions,
}
impl ActionExecutor for BitcoinClaimer {
    fn execute<'a>(&'a self, id: &'a str, action: &'a ActionKind) -> ExecutionFuture<'a> {
        Box::pin(async move {
            if *action != ActionKind::Claim {
                return Err(Error::InvalidSwap(
                    "this adapter only claims Bitcoin".into(),
                ));
            }
            let receipt = self
                .client
                .claim_bitcoin(id, self.backend.as_ref(), self.options.clone())
                .await?;
            Ok(ExecutionResult::Submitted {
                reference: receipt.transaction.compute_txid().to_string(),
            })
        })
    }
}
#[cfg(feature = "websocket")]
pub async fn feed_status(
    mut subscription: lendaswap_sdk::subscription::StatusSubscription,
    tracker: SwapTracker,
    cancellation: lendaswap_sdk::control::Cancellation,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = cancellation.cancelled() => break,
            event = subscription.recv() => match event {
                Some(event) => tracker.status_hint(&event)?,
                None => break,
            }
        }
    }
    subscription.shutdown().await
}
fn main() {
    println!(
        "Tracking template: configure readers, restore journals, subscribe to events, and retain the worker until shutdown. Automatic execution is opt-in."
    );
}
