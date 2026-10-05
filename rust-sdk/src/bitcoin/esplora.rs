use super::*;
use std::collections::HashSet;

/// Mempool's recommended fees, which the swap server spends at too.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendedFees {
    fastest_fee: f64,
    half_hour_fee: f64,
    hour_fee: f64,
    economy_fee: f64,
    minimum_fee: f64,
}

/// Bitcoin Core's default minimum relay fee rate, in sat/vB.
const MIN_RELAY_SAT_VB: f64 = 1.0;

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

    /// Mempool's recommended fees at the block targets the swap server reads them
    /// for (swap/src/electrum.rs), or `None` from a server without Mempool's API,
    /// such as blockstream.info. Mempool serves no `fee-estimates` over an Electrum
    /// backend and has deprecated it on mempool.space, so that is only the fallback.
    async fn recommended_fees(&self) -> Result<Option<Vec<(u16, f64)>>> {
        let response = self
            .http
            .get(format!("{}/v1/fees/recommended", self.base_url))
            .send()
            .await
            .map_err(transport)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let fees: RecommendedFees = response
            .error_for_status()
            .map_err(transport)?
            .json()
            .await
            .map_err(decode)?;
        Ok(Some(vec![
            (1, fees.fastest_fee),
            (3, fees.half_hour_fee),
            (6, fees.hour_fee),
            (25, fees.economy_fee),
            (144, fees.minimum_fee),
        ]))
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
pub(crate) async fn scripthash_history(
    client: &esplora_client::AsyncClient,
    http: &reqwest::Client,
    base_url: &str,
    script: &bitcoin::Script,
) -> Result<Vec<esplora_client::Tx>> {
    // /txs lists mempool entries before confirmed ones and some servers cap the whole
    // page, so confirmed history is paged on its own: unconfirmed spam must not hide
    // a confirmed funding.
    let mut entries = client
        .scripthash_txs(script, None)
        .await
        .map_err(transport)?;
    let hash = bitcoin::hashes::sha256::Hash::hash(script.as_bytes());
    // esplora-client only wraps the continuation form of /txs/chain.
    let chain = http
        .get(format!("{base_url}/scripthash/{hash:x}/txs/chain"))
        .send()
        .await
        .map_err(transport)?;
    // Mempool on an electrum backend serves no /txs/chain. Its /txs pages the whole
    // history, mempool first, ten transactions a request through after_txid, so the
    // first page is continued to the end: one request per ten transactions.
    let electrum = chain.status() == reqwest::StatusCode::NOT_FOUND;
    let (mut page, page_size): (Vec<esplora_client::Tx>, _) = if electrum {
        (std::mem::take(&mut entries), 10)
    } else {
        (
            chain
                .error_for_status()
                .map_err(transport)?
                .json()
                .await
                .map_err(decode)?,
            25,
        )
    };
    let mut last_seen = None;
    loop {
        let next = page.last().map(|tx| tx.txid);
        let full = page.len() >= page_size;
        entries.extend(page);
        if !full {
            break;
        }
        let Some(last) = next.filter(|next| Some(*next) != last_seen) else {
            return Err(invalid("Esplora history pagination did not advance"));
        };
        last_seen = Some(last);
        page = if electrum {
            http.get(format!(
                "{base_url}/scripthash/{hash:x}/txs?after_txid={last}"
            ))
            .send()
            .await
            .map_err(transport)?
            .error_for_status()
            .map_err(transport)?
            .json()
            .await
            .map_err(decode)?
        } else {
            client
                .scripthash_txs(script, Some(last))
                .await
                .map_err(transport)?
        };
    }
    let mut seen = HashSet::new();
    entries.retain(|tx| seen.insert(tx.txid));
    Ok(entries)
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
                return Err(chain_changed("Bitcoin tip changed during observation"));
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
            let entries =
                scripthash_history(&self.client, &self.http, &self.base_url, &script).await?;
            let listed: HashSet<Txid> = entries.iter().map(|tx| tx.txid).collect();
            let listed_spends: HashSet<(Txid, u32)> = entries
                .iter()
                .flat_map(|tx| tx.vin.iter().map(|input| (input.txid, input.vout)))
                .collect();
            let mut unlisted_spenders: Vec<Txid> = vec![];
            for tx in entries {
                let confirmed_height = if tx.status.confirmed {
                    let h = tx
                        .status
                        .block_height
                        .ok_or_else(|| invalid("confirmed Bitcoin transaction has no height"))?;
                    if h > height
                        || Some(self.client.get_block_hash(h).await.map_err(transport)?)
                            != tx.status.block_hash
                    {
                        return Err(chain_changed(
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
                    } else if let Some(spender) =
                        status.txid.filter(|spender| !listed.contains(spender))
                    {
                        // The capped mempool listing can leave out a spend of ours,
                        // for example behind dust spam, so follow the outspend. Only
                        // an unconfirmed spender can be missing from a consistent read.
                        if status.status.is_some_and(|s| s.confirmed)
                            || listed_spends.contains(&(tx.txid, vout))
                        {
                            return Err(chain_changed(
                                "Bitcoin history changed during observation",
                            ));
                        }
                        if !unlisted_spenders.contains(&spender) {
                            unlisted_spenders.push(spender);
                        }
                    }
                }
            }
            for spender in unlisted_spenders {
                if let Some(raw) = self.fetch_transaction(spender).await? {
                    if raw.compute_txid() != spender {
                        return Err(invalid("Bitcoin spending transaction ID mismatch"));
                    }
                    transactions.push(BitcoinHistoryTransaction {
                        transaction: raw,
                        confirmed_height: None,
                    });
                }
            }
            if self.client.get_tip_hash().await.map_err(transport)? != tip {
                return Err(chain_changed("Bitcoin tip changed during observation"));
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
            let estimates: Vec<(u16, f64)> = match self.recommended_fees().await? {
                Some(estimates) => estimates,
                None => self
                    .client
                    .get_fee_estimates()
                    .await
                    .map_err(transport)?
                    .into_iter()
                    .collect(),
            };
            let rate = estimates
                .iter()
                .filter(|(blocks, _)| *blocks <= target)
                .max_by_key(|(blocks, _)| *blocks)
                .or_else(|| estimates.iter().min_by_key(|(blocks, _)| *blocks))
                .map(|(_, rate)| *rate)
                .ok_or_else(|| invalid("no Bitcoin fee estimate available"))?;
            // Esplora estimates go below the minimum relay fee most nodes keep, so a
            // spend at them would not propagate. Non-positive rates still fail.
            fee_from_sat_vb(if rate > 0.0 {
                rate.max(MIN_RELAY_SAT_VB)
            } else {
                rate
            })
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

    // Bodies captured on 2026-10-05: `fee-estimates` from the regtest Mempool over
    // Fulcrum (404) and from mempool.space (203, the deprecation now in a header),
    // and blockstream.info's 404 for Mempool's recommended fees and its estimates.
    const REGTEST_FEE_ESTIMATES_404: &str = "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n</head>\n<body>\n<pre>Cannot GET /api/v1/fee-estimates</pre>\n</body>\n</html>\n";
    const MEMPOOL_SPACE_FEE_ESTIMATES_203: &str = r#"{"1":1.269,"2":1.269,"3":0.786,"4":0.786,"5":0.786,"6":0.396,"7":0.396,"8":0.396,"9":0.396,"10":0.396,"11":0.396,"12":0.396,"13":0.396,"14":0.396,"15":0.396,"16":0.396,"17":0.396,"18":0.396,"19":0.396,"20":0.396,"21":0.396,"22":0.396,"23":0.396,"24":0.396,"25":0.396,"144":0.2,"504":0.2,"1008":0.1}"#;
    const BLOCKSTREAM_RECOMMENDED_404: &str = r#"endpoint does not exist "/v1/fees/recommended""#;
    const BLOCKSTREAM_FEE_ESTIMATES: &str = r#"{"6":0.381,"7":0.381,"2":1.066,"5":0.42200000000000004,"12":0.284,"11":0.284,"18":0.284,"20":0.284,"3":1.066,"17":0.284,"21":0.284,"15":0.284,"144":0.25,"9":0.301,"1":1.066,"13":0.284,"16":0.284,"24":0.284,"19":0.284,"8":0.301,"23":0.284,"14":0.284,"25":0.27499999999999997,"504":0.25,"22":0.284,"1008":0.09999999999999999,"10":0.301,"4":0.42200000000000004}"#;
    /// mempool.space's body on 2026-10-04, shortened: its six-block rate and its
    /// deprecation entry as captured, which no strict parse of `fee-estimates`
    /// accepts.
    const MEMPOOL_SPACE_FEE_ESTIMATES_WARNING: &str = r#"{"1":2.012,"3":1.517,"6":0.493,"144":0.2,"warning":"This endpoint is deprecated and will be removed in a future release. Please use /api/v1/fees/precise instead."}"#;

    fn sat_vb(rate: u64) -> FeeRate {
        FeeRate::from_sat_per_kwu(rate * 250)
    }

    #[tokio::test]
    async fn fee_rate_reads_mempool_recommended_fees_before_fee_estimates() {
        for (status, body) in [
            (404, REGTEST_FEE_ESTIMATES_404),
            (200, MEMPOOL_SPACE_FEE_ESTIMATES_WARNING),
            (203, MEMPOOL_SPACE_FEE_ESTIMATES_203),
        ] {
            let server = MockServer::start().await;
            Mock::given(path("/v1/fees/recommended"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "fastestFee": 5, "halfHourFee": 4, "hourFee": 3, "economyFee": 2, "minimumFee": 1
                })))
                .mount(&server)
                .await;
            Mock::given(path("/fee-estimates"))
                .respond_with(ResponseTemplate::new(status).set_body_string(body))
                .expect(0)
                .mount(&server)
                .await;
            let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
            for (target, rate) in [(6, 3), (1, 5), (3, 4), (25, 2), (0, 5), (1008, 1)] {
                assert_eq!(
                    backend.fee_rate(target).await.unwrap(),
                    sat_vb(rate),
                    "{target}"
                );
            }
        }
    }

    #[tokio::test]
    async fn fee_rate_falls_back_to_fee_estimates_without_mempools_api() {
        // A map without an entry for every target, above the relay floor.
        let server = MockServer::start().await;
        Mock::given(path("/v1/fees/recommended"))
            .respond_with(ResponseTemplate::new(404).set_body_string(BLOCKSTREAM_RECOMMENDED_404))
            .mount(&server)
            .await;
        Mock::given(path("/fee-estimates"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"1": 4.0, "3": 3.5, "6": 2.004, "144": 1.0})),
            )
            .mount(&server)
            .await;
        let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
        assert_eq!(
            backend.fee_rate(6).await.unwrap(),
            FeeRate::from_sat_per_kwu(501)
        );

        // blockstream.info's estimates, mostly below the relay floor.
        let server = MockServer::start().await;
        Mock::given(path("/v1/fees/recommended"))
            .respond_with(ResponseTemplate::new(404).set_body_string(BLOCKSTREAM_RECOMMENDED_404))
            .mount(&server)
            .await;
        Mock::given(path("/fee-estimates"))
            .respond_with(ResponseTemplate::new(200).set_body_string(BLOCKSTREAM_FEE_ESTIMATES))
            .mount(&server)
            .await;
        let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
        assert_eq!(backend.fee_rate(6).await.unwrap(), sat_vb(1));
        assert_eq!(
            backend.fee_rate(1).await.unwrap(),
            FeeRate::from_sat_per_kwu(267)
        );
    }

    #[tokio::test]
    async fn fee_rate_never_goes_below_the_minimum_relay_fee() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/fees/recommended"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "fastestFee": 2, "halfHourFee": 1.5, "hourFee": 0.5, "economyFee": 0.2, "minimumFee": 0.1
            })))
            .mount(&server)
            .await;
        let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
        assert_eq!(backend.fee_rate(6).await.unwrap(), sat_vb(1));
        assert_eq!(
            backend.fee_rate(3).await.unwrap(),
            FeeRate::from_sat_per_kwu(375)
        );
    }

    #[tokio::test]
    async fn fee_rate_fails_on_any_other_recommended_fees_failure() {
        for (status, body) in [(503, "busy"), (200, "{}"), (200, "null")] {
            let server = MockServer::start().await;
            Mock::given(path("/v1/fees/recommended"))
                .respond_with(ResponseTemplate::new(status).set_body_string(body))
                .mount(&server)
                .await;
            Mock::given(path("/fee-estimates"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"6": 2.0})))
                .expect(0)
                .mount(&server)
                .await;
            let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
            let error = backend.fee_rate(6).await.unwrap_err();
            if status == 503 {
                assert!(matches!(error, Error::Transport(_)), "{error}");
            } else {
                assert!(matches!(error, Error::Decode(_)), "{body}: {error}");
            }
        }
    }
}
