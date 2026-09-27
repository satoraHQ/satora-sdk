use super::*;
use electrum_client::Client as ElectrumClient;
use electrum_client::ConfigBuilder;
use electrum_client::ElectrumApi;
use std::sync::Arc;

/// Electrum TCP/TLS reader. Blocking socket work runs on Tokio's blocking pool;
/// socket timeouts bound abandoned tasks when an outer SDK deadline expires.
pub struct ElectrumBackend {
    client: Arc<ElectrumClient>,
    network: Network,
}
impl ElectrumBackend {
    pub async fn connect(url: String, network: Network, timeout_seconds: u8) -> Result<Self> {
        if timeout_seconds == 0 {
            return Err(invalid("Electrum timeout must be positive"));
        }
        crate::crypto_init::ensure_default_provider_installed();
        let client = tokio::task::spawn_blocking(move || {
            let config = ConfigBuilder::new()
                .timeout(Some(timeout_seconds))
                .retry(1)
                .build();
            let client = ElectrumClient::from_config(&url, config).map_err(transport)?;
            if client.block_header(0).map_err(transport)?.block_hash()
                != bitcoin::blockdata::constants::genesis_block(network).block_hash()
            {
                return Err(invalid(
                    "Electrum genesis block does not match the configured network",
                ));
            }
            Ok(client)
        })
        .await
        .map_err(transport)??;
        Ok(Self {
            client: Arc::new(client),
            network,
        })
    }
}
// Servers report an unknown txid differently: bitcoind's -5 at the top level,
// ElectrumX and Fulcrum wrap it in a daemon error, Blockstream electrs sends a string.
// The markers stay specific because a bare "not found" also matches "method not found".
fn is_missing_transaction(error: &serde_json::Value) -> bool {
    if error["code"] == -5 {
        return true;
    }
    let text = error.to_string().to_ascii_lowercase();
    [
        "no such mempool or blockchain transaction",
        "missing transaction",
        "transaction not found",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}
impl BitcoinBackend for ElectrumBackend {
    fn network(&self) -> Network {
        self.network
    }
    fn observe<'a>(&'a self, address: &'a Address) -> ChainFuture<'a, BitcoinObservation> {
        let client = self.client.clone();
        let script = address.script_pubkey();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let head = client.block_headers_subscribe().map_err(transport)?;
                let height = u32::try_from(head.height).map_err(decode)?;
                let tip = head.header.block_hash();
                let mut times = vec![];
                let mut expected = tip;
                for h in (height.saturating_sub(10)..=height).rev() {
                    let header = client.block_header(h as usize).map_err(transport)?;
                    if header.block_hash() != expected {
                        return Err(invalid("Electrum header chain changed"));
                    }
                    times.push(header.time);
                    expected = header.prev_blockhash;
                }
                times.sort_unstable();
                let median_time_past = times[times.len() / 2];
                let mut utxos = vec![];
                for u in client.script_list_unspent(&script).map_err(transport)? {
                    let tx = client.transaction_get(&u.tx_hash).map_err(transport)?;
                    if tx.compute_txid() != u.tx_hash {
                        return Err(invalid("Electrum funding transaction ID mismatch"));
                    }
                    let output = tx.output.get(u.tx_pos).ok_or_else(|| {
                        invalid("Electrum outpoint is outside the funding transaction")
                    })?;
                    if output.script_pubkey != script || output.value.to_sat() != u.value {
                        return Err(invalid("Electrum funding output mismatch"));
                    }
                    let confirmed_height = if u.height == 0 {
                        None
                    } else {
                        Some(u32::try_from(u.height).map_err(decode)?)
                    };
                    if confirmed_height.is_some_and(|h| h > height) {
                        return Err(invalid("Electrum funding height exceeds tip"));
                    }
                    utxos.push(BitcoinUtxo {
                        outpoint: OutPoint {
                            txid: u.tx_hash,
                            vout: u32::try_from(u.tx_pos).map_err(decode)?,
                        },
                        output: output.clone(),
                        confirmed_height,
                    });
                }
                if client
                    .block_headers_subscribe()
                    .map_err(transport)?
                    .header
                    .block_hash()
                    != tip
                {
                    return Err(invalid("Electrum tip changed during observation"));
                }
                Ok(BitcoinObservation {
                    tip,
                    height,
                    median_time_past,
                    utxos,
                })
            })
            .await
            .map_err(transport)?
        })
    }
    fn history<'a>(&'a self, address: &'a Address) -> ChainFuture<'a, BitcoinHistory> {
        Box::pin(async move {
            let observation = self.observe(address).await?;
            let client = self.client.clone();
            let script = address.script_pubkey();
            let tip = observation.tip;
            let height = observation.height;
            let transactions = tokio::task::spawn_blocking(move || {
                let mut transactions = Vec::new();
                for entry in client.script_get_history(&script).map_err(transport)? {
                    let confirmed_height = if entry.height > 0 {
                        let height = u32::try_from(entry.height).map_err(decode)?;
                        Some(height)
                    } else {
                        None
                    };
                    if confirmed_height.is_some_and(|h| h > height) {
                        return Err(invalid("Electrum history height exceeds tip"));
                    }
                    let transaction = client.transaction_get(&entry.tx_hash).map_err(transport)?;
                    if transaction.compute_txid() != entry.tx_hash {
                        return Err(invalid("Electrum history transaction ID mismatch"));
                    }
                    transactions.push(BitcoinHistoryTransaction {
                        transaction,
                        confirmed_height,
                    });
                }
                if client
                    .block_headers_subscribe()
                    .map_err(transport)?
                    .header
                    .block_hash()
                    != tip
                {
                    return Err(invalid("Electrum tip changed during history read"));
                }
                Ok(transactions)
            })
            .await
            .map_err(transport)??;
            Ok(BitcoinHistory {
                observation,
                transactions,
            })
        })
    }
    fn transaction(&self, id: Txid) -> ChainFuture<'_, Option<Transaction>> {
        let client = self.client.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || match client.transaction_get(&id) {
                Ok(tx) if tx.compute_txid() == id => Ok(Some(tx)),
                Ok(_) => Err(invalid("Electrum transaction ID mismatch")),
                Err(electrum_client::Error::Protocol(value)) if is_missing_transaction(&value) => {
                    Ok(None)
                }
                Err(error) => Err(transport(error)),
            })
            .await
            .map_err(transport)?
        })
    }
    fn fee_rate(&self, target: u16) -> ChainFuture<'_, FeeRate> {
        let client = self.client.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                // Electrum returns BTC/kB; 1 BTC/kB = 100000 sat/vB.
                super::esplora::fee_from_sat_vb(
                    client
                        .estimate_fee(usize::from(target.max(1)))
                        .map_err(transport)?
                        * 100_000.0,
                )
            })
            .await
            .map_err(transport)?
        })
    }
    fn broadcast<'a>(&'a self, tx: &'a Transaction) -> ChainFuture<'a, Txid> {
        let client = self.client.clone();
        let tx = tx.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let id = client.transaction_broadcast(&tx).map_err(transport)?;
                if id != tx.compute_txid() {
                    return Err(invalid("Electrum broadcast transaction ID mismatch"));
                }
                Ok(id)
            })
            .await
            .map_err(transport)?
        })
    }
}
