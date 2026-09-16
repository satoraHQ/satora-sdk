use super::*;
use std::collections::HashSet;

pub struct EsploraBackend {
    client: esplora_client::AsyncClient,
    network: Network,
}
impl EsploraBackend {
    pub fn new(url: &str, network: Network) -> Result<Self> {
        crate::crypto_init::ensure_default_provider_installed();
        Ok(Self {
            client: esplora_client::Builder::new(url)
                .build_async()
                .map_err(transport)?,
            network,
        })
    }
    async fn verify_network(&self) -> Result<()> {
        if self.client.get_block_hash(0).await.map_err(transport)?
            != bitcoin::blockdata::constants::genesis_block(self.network).block_hash()
        {
            return Err(invalid(
                "Esplora genesis block does not match the configured network",
            ));
        }
        Ok(())
    }
}
impl BitcoinBackend for EsploraBackend {
    fn network(&self) -> Network {
        self.network
    }
    fn observe<'a>(&'a self, address: &'a Address) -> ChainFuture<'a, BitcoinObservation> {
        Box::pin(async move { Ok(self.history(address).await?.observation) })
    }
    fn history<'a>(&'a self, address: &'a Address) -> ChainFuture<'a, BitcoinHistory> {
        Box::pin(async move {
            self.verify_network().await?;
            let tip = self.client.get_tip_hash().await.map_err(transport)?;
            let height = self.client.get_height().await.map_err(transport)?;
            if self
                .client
                .get_block_hash(height)
                .await
                .map_err(transport)?
                != tip
            {
                return Err(invalid("Bitcoin tip changed during observation"));
            }
            let mut times = vec![];
            let mut cursor = tip;
            for _ in 0..=height.min(10) {
                let header = self
                    .client
                    .get_header_by_hash(&cursor)
                    .await
                    .map_err(transport)?;
                if header.block_hash() != cursor {
                    return Err(invalid("Bitcoin header hash mismatch"));
                }
                times.push(header.time);
                cursor = header.prev_blockhash;
            }
            times.sort_unstable();
            let median_time_past = times[times.len() / 2];
            let script = address.script_pubkey();
            let mut utxos = vec![];
            let mut transactions = vec![];
            let mut seen = HashSet::new();
            let mut last_seen = None;
            loop {
                let page = self
                    .client
                    .scripthash_txs(&script, last_seen)
                    .await
                    .map_err(transport)?;
                let confirmed: Vec<_> = page
                    .iter()
                    .filter(|tx| tx.status.confirmed)
                    .map(|tx| tx.txid)
                    .collect();
                let next = confirmed.last().copied();
                for tx in page {
                    if !seen.insert(tx.txid) {
                        continue;
                    }
                    let confirmed_height = if tx.status.confirmed {
                        let h = tx.status.block_height.ok_or_else(|| {
                            invalid("confirmed Bitcoin transaction has no height")
                        })?;
                        if h > height
                            || Some(self.client.get_block_hash(h).await.map_err(transport)?)
                                != tx.status.block_hash
                        {
                            return Err(invalid(
                                "Bitcoin funding block is not on the observed chain",
                            ));
                        }
                        Some(h)
                    } else {
                        None
                    };
                    let raw = self
                        .client
                        .get_tx(&tx.txid)
                        .await
                        .map_err(transport)?
                        .ok_or_else(|| invalid("Bitcoin funding transaction missing"))?;
                    if raw.compute_txid() != tx.txid {
                        return Err(invalid("Bitcoin funding transaction ID mismatch"));
                    }
                    transactions.push(BitcoinHistoryTransaction {
                        transaction: raw.clone(),
                        confirmed_height,
                    });
                    for (vout, output) in raw
                        .output
                        .iter()
                        .enumerate()
                        .filter(|(_, o)| o.script_pubkey == script)
                    {
                        let status = self
                            .client
                            .get_output_status(&tx.txid, vout as u64)
                            .await
                            .map_err(transport)?
                            .ok_or_else(|| invalid("Bitcoin output spend status unknown"))?;
                        if !status.spent {
                            utxos.push(BitcoinUtxo {
                                outpoint: OutPoint {
                                    txid: tx.txid,
                                    vout: u32::try_from(vout).map_err(decode)?,
                                },
                                output: output.clone(),
                                confirmed_height,
                            });
                        }
                    }
                }
                if confirmed.len() < 25 {
                    break;
                }
                if next.is_none() || next == last_seen {
                    return Err(invalid("Esplora history pagination did not advance"));
                }
                last_seen = next;
            }
            if self.client.get_tip_hash().await.map_err(transport)? != tip {
                return Err(invalid("Bitcoin tip changed during observation"));
            }
            Ok(BitcoinHistory {
                observation: BitcoinObservation {
                    tip,
                    height,
                    median_time_past,
                    utxos,
                },
                transactions,
            })
        })
    }
    fn transaction(&self, id: Txid) -> ChainFuture<'_, Option<Transaction>> {
        Box::pin(async move {
            self.verify_network().await?;
            let tx = self.client.get_tx(&id).await.map_err(transport)?;
            if tx.as_ref().is_some_and(|t| t.compute_txid() != id) {
                return Err(invalid("Bitcoin transaction ID mismatch"));
            }
            Ok(tx)
        })
    }
    fn fee_rate(&self, target: u16) -> ChainFuture<'_, FeeRate> {
        Box::pin(async move {
            let estimates = self.client.get_fee_estimates().await.map_err(transport)?;
            let rate = estimates
                .iter()
                .filter(|(blocks, _)| **blocks <= target)
                .max_by_key(|(blocks, _)| **blocks)
                .or_else(|| estimates.iter().min_by_key(|(blocks, _)| **blocks))
                .map(|(_, rate)| *rate)
                .ok_or_else(|| invalid("no Bitcoin fee estimate available"))?;
            fee_from_sat_vb(rate)
        })
    }
    fn broadcast<'a>(&'a self, tx: &'a Transaction) -> ChainFuture<'a, Txid> {
        Box::pin(async move {
            self.verify_network().await?;
            self.client.broadcast(tx).await.map_err(transport)?;
            Ok(tx.compute_txid())
        })
    }
}

pub(super) fn fee_from_sat_vb(rate: f64) -> Result<FeeRate> {
    if !rate.is_finite() || rate <= 0.0 || rate > 1_000_000.0 {
        return Err(invalid("invalid Bitcoin fee estimate"));
    }
    Ok(FeeRate::from_sat_per_kwu((rate * 250.0).ceil() as u64))
}
