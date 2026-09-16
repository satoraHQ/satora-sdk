use super::*;
use std::collections::HashSet;

pub struct EsploraBackend {
    client: esplora_client::AsyncClient,
    network: Network,
    http: reqwest::Client,
    base_url: String,
}
impl EsploraBackend {
    pub fn new(url: &str, network: Network) -> Result<Self> {
        crate::crypto_init::ensure_default_provider_installed();
        Ok(Self {
            client: esplora_client::Builder::new(url)
                .build_async()
                .map_err(transport)?,
            network,
            http: reqwest::Client::new(),
            base_url: url.trim_end_matches('/').into(),
        })
    }
    async fn fetch_transaction(&self, id: Txid) -> Result<Option<Transaction>> {
        let transaction = match self.client.get_tx(&id).await.map_err(transport)? {
            Some(tx) => Some(tx),
            None => {
                // Mempool exposes transaction hex but may omit Esplora's raw endpoint.
                let response = self
                    .http
                    .get(format!("{}/tx/{id}/hex", self.base_url))
                    .send()
                    .await
                    .map_err(transport)?;
                if response.status() == reqwest::StatusCode::NOT_FOUND {
                    return Ok(None);
                }
                let text = response
                    .error_for_status()
                    .map_err(transport)?
                    .text()
                    .await
                    .map_err(transport)?;
                Some(
                    bitcoin::consensus::deserialize::<Transaction>(
                        &hex::decode(text.trim()).map_err(decode)?,
                    )
                    .map_err(decode)?,
                )
            }
        };
        if transaction
            .as_ref()
            .is_some_and(|tx| tx.compute_txid() != id)
        {
            return Err(invalid("Bitcoin transaction ID mismatch"));
        }
        Ok(transaction)
    }

    async fn output_status(&self, txid: Txid, vout: u32) -> Result<esplora_client::OutputStatus> {
        if let Some(status) = self
            .client
            .get_output_status(&txid, u64::from(vout))
            .await
            .map_err(transport)?
        {
            return Ok(status);
        }
        // Missing status must never be interpreted as an unspent output.
        let statuses: Vec<esplora_client::OutputStatus> = self
            .http
            .get(format!("{}/tx/{txid}/outspends", self.base_url))
            .send()
            .await
            .map_err(transport)?
            .error_for_status()
            .map_err(transport)?
            .json()
            .await
            .map_err(decode)?;
        statuses
            .into_iter()
            .nth(vout as usize)
            .ok_or_else(|| invalid("Bitcoin output spend status unknown"))
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
                        .fetch_transaction(tx.txid)
                        .await?
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
                        let vout = u32::try_from(vout).map_err(decode)?;
                        let status = self.output_status(tx.txid, vout).await?;
                        if !status.spent {
                            utxos.push(BitcoinUtxo {
                                outpoint: OutPoint {
                                    txid: tx.txid,
                                    vout,
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
            self.fetch_transaction(id).await
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
            // Mempool's body parser requires an explicit text content type.
            let response = self
                .http
                .post(format!("{}/tx", self.base_url))
                .header(reqwest::header::CONTENT_TYPE, "text/plain")
                .body(bitcoin::consensus::encode::serialize_hex(tx))
                .send()
                .await
                .map_err(transport)?
                .error_for_status()
                .map_err(transport)?
                .text()
                .await
                .map_err(transport)?;
            let txid = Txid::from_str(response.trim()).map_err(decode)?;
            if txid != tx.compute_txid() {
                return Err(invalid(
                    "Bitcoin broadcast returned a different transaction ID",
                ));
            }
            Ok(txid)
        })
    }
}

pub(super) fn fee_from_sat_vb(rate: f64) -> Result<FeeRate> {
    if !rate.is_finite() || rate <= 0.0 || rate > 1_000_000.0 {
        return Err(invalid("invalid Bitcoin fee estimate"));
    }
    Ok(FeeRate::from_sat_per_kwu((rate * 250.0).ceil() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;

    #[tokio::test]
    async fn broadcast_sends_text_and_checks_the_returned_transaction_id() {
        use wiremock::matchers::body_string;
        use wiremock::matchers::header;
        use wiremock::matchers::method;
        let genesis = bitcoin::blockdata::constants::genesis_block(Network::Regtest);
        let tx = &genesis.txdata[0];
        for returned in [tx.compute_txid(), Txid::all_zeros()] {
            let server = MockServer::start().await;
            Mock::given(path("/block-height/0"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_string(genesis.block_hash().to_string()),
                )
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/tx"))
                .and(header("content-type", "text/plain"))
                .and(body_string(bitcoin::consensus::encode::serialize_hex(tx)))
                .respond_with(ResponseTemplate::new(200).set_body_string(returned.to_string()))
                .expect(1)
                .mount(&server)
                .await;
            let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
            let result = backend.broadcast(tx).await;
            if returned == tx.compute_txid() {
                assert_eq!(result.unwrap(), returned);
            } else {
                assert!(result.is_err());
            }
        }
    }

    #[tokio::test]
    async fn mempool_hex_fallback_validates_transaction_identity() {
        let server = MockServer::start().await;
        let tx = bitcoin::blockdata::constants::genesis_block(Network::Regtest).txdata[0].clone();
        let id = tx.compute_txid();
        let wrong_id = Txid::all_zeros();
        for requested in [id, wrong_id] {
            Mock::given(path(format!("/tx/{requested}/hex")))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(bitcoin::consensus::encode::serialize_hex(&tx)),
                )
                .mount(&server)
                .await;
        }
        let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
        assert_eq!(backend.fetch_transaction(id).await.unwrap(), Some(tx));
        assert!(backend.fetch_transaction(wrong_id).await.is_err());
    }

    #[tokio::test]
    async fn unavailable_or_malformed_transaction_is_not_accepted() {
        for status in [404, 200, 500] {
            let server = MockServer::start().await;
            let id = Txid::all_zeros();
            Mock::given(path(format!("/tx/{id}/hex")))
                .respond_with(ResponseTemplate::new(status).set_body_string("invalid hex"))
                .mount(&server)
                .await;
            let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
            let result = backend.fetch_transaction(id).await;
            if status == 404 {
                assert!(result.unwrap().is_none());
            } else {
                assert!(result.is_err());
            }
        }
    }

    #[tokio::test]
    async fn bulk_spend_status_preserves_spent_and_rejects_unknown_outputs() {
        let server = MockServer::start().await;
        let id = Txid::all_zeros();
        Mock::given(path(format!("/tx/{id}/outspends")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"spent": false}, {"spent": true, "txid": "11".repeat(32), "vin": 0}
            ])))
            .mount(&server)
            .await;
        let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
        assert!(!backend.output_status(id, 0).await.unwrap().spent);
        assert!(backend.output_status(id, 1).await.unwrap().spent);
        assert!(backend.output_status(id, 2).await.is_err());
        assert!(
            backend
                .output_status(Txid::from_byte_array([42; 32]), 0)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn existing_esplora_endpoints_do_not_use_fallbacks() {
        let server = MockServer::start().await;
        let tx = bitcoin::blockdata::constants::genesis_block(Network::Regtest).txdata[0].clone();
        let id = tx.compute_txid();
        Mock::given(path(format!("/tx/{id}/raw")))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(bitcoin::consensus::serialize(&tx)),
            )
            .mount(&server)
            .await;
        Mock::given(path(format!("/tx/{id}/outspend/0")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"spent": false})))
            .mount(&server)
            .await;
        for suffix in ["hex", "outspends"] {
            Mock::given(path(format!("/tx/{id}/{suffix}")))
                .respond_with(ResponseTemplate::new(500))
                .expect(0)
                .mount(&server)
                .await;
        }
        let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
        assert_eq!(backend.fetch_transaction(id).await.unwrap(), Some(tx));
        assert!(!backend.output_status(id, 0).await.unwrap().spent);
    }
}
