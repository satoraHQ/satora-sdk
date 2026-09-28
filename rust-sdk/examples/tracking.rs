//! Wire a dedicated contract manager to persistent tracking, optionally with the
//! stock `AutoClaimer` for swaps that opt in to automatic claims.
use lendaswap_sdk::Client;
use lendaswap_sdk::Error;
use lendaswap_sdk::Result;
use lendaswap_sdk::actions::SwapActions;
use lendaswap_sdk::bitcoin::BitcoinBackend;
use lendaswap_sdk::bitcoin::EsploraBackend;
use lendaswap_sdk::observation::BitcoinReader;
use lendaswap_sdk::observation::ContractManager;
use lendaswap_sdk::observation::ContractReader;
use lendaswap_sdk::observation::ManagerOptions;
use lendaswap_sdk::observation::ReferenceOptions;
use lendaswap_sdk::tracker::ActionExecutor;
use lendaswap_sdk::tracker::AutoClaimConfig;
use lendaswap_sdk::tracker::AutoClaimer;
use lendaswap_sdk::tracker::BitcoinAutoClaim;
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
/// Configure EVM scan start, coordinator and locked token in `references`. Pass
/// `claimer` to claim swaps tracked with `auto_claim`.
pub async fn track_stored(
    client: Client,
    reader: Arc<dyn ContractReader>,
    id: &str,
    references: ReferenceOptions,
    claimer: Option<Arc<dyn ActionExecutor>>,
) -> Result<(SwapTracker, TrackerWorker)> {
    let options = TrackingOptions {
        auto_claim: claimer.is_some(),
        ..Default::default()
    };
    let manager = ContractManager::new(reader, ManagerOptions::default())?;
    let tracker = SwapTracker::new(client.clone(), manager, TrackerOptions::default(), claimer)?;
    let failures = tracker.restore().await?;
    if !failures.is_empty() {
        return Err(Error::Storage(format!("restore failures: {failures:?}")));
    }
    // Stored swaps without a journal, such as one created just before a crash. The
    // EVM scan start, sender and lock token in `references` belong to `id` alone.
    // A failure (an unobservable leg, a full tracker) recurs until it is fixed.
    let failures = tracker
        .adopt_untracked(|stored| {
            let references = if stored.response.id() == id {
                references.clone()
            } else {
                references.shared_policy()
            };
            Some((references, options.clone()))
        })
        .await?;
    for (id, error) in failures {
        tracing::warn!(%id, %error, "swap not adopted for tracking");
    }
    // A retired swap keeps its journal and is resumed; a swap adoption skipped is
    // tracked directly.
    if tracker.record(id).await.is_err() {
        let journaled = client
            .get_stored_swap(id)?
            .is_some_and(|stored| stored.metadata.tracking.is_some());
        if journaled {
            tracker.resume(id).await?;
        } else {
            let contracts = client.contract_references(id, &references)?;
            tracker.track(id, contracts, options).await?;
        }
    }
    let worker = tracker.start()?;
    Ok((tracker, worker))
}
/// Claims EVM to Bitcoin swaps, and Arkade swaps through the client's Arkade
/// config, to the destinations recorded when they were created. A Bitcoin claim
/// waits for `Client::bitcoin_min_confirmations`, which is 0 unless set, so by
/// default it spends a mempool funding. EVM claims also need
/// `AutoClaimConfig::evm` with the allowed contracts per chain.
pub fn bitcoin_claimer(
    client: Client,
    backend: Arc<dyn BitcoinBackend>,
) -> Result<Arc<dyn ActionExecutor>> {
    Ok(Arc::new(AutoClaimer::new(
        client,
        AutoClaimConfig {
            bitcoin: Some(BitcoinAutoClaim::new(
                backend,
                bitcoin::Amount::from_sat(5_000),
            )),
            ..Default::default()
        },
    )?))
}
/// Surface what the user must act on, as the TypeScript `onActionRequired` does.
pub async fn watch_required_actions(client: Client, tracker: SwapTracker) -> Result<()> {
    use tokio::sync::broadcast::error::RecvError;
    let report = |id: &str, actions: &SwapActions| {
        if actions.needs_user() {
            tracing::info!(swap_id = %id, ?actions, "action required");
        }
    };
    let mut events = tracker.events();
    loop {
        match events.recv().await {
            Ok(event) => {
                if let Some(actions) = &event.actions {
                    report(&event.swap_id, actions);
                }
            }
            // Unchanged actions are never emitted again, so a dropped event is only
            // recovered by reading every tracked swap's last actions.
            Err(RecvError::Lagged(_)) => {
                for stored in client.list_all_swaps()? {
                    let id = stored.response.id();
                    if let Ok(Some(actions)) = tracker.actions(id) {
                        report(id, &actions);
                    }
                }
            }
            Err(RecvError::Closed) => return Ok(()),
        }
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
