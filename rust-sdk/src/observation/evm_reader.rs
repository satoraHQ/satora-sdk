use super::*;
use crate::bitcoin::chain_changed;
use crate::evm::B256;
use crate::evm::U256;
use crate::evm::funding::SwapCreated;
use crate::evm::htlc::EvmHtlc;
use crate::evm::htlc::SwapRedeemed;
use crate::evm::htlc::SwapRefunded;
use alloy::providers::Provider;
use alloy::providers::RootProvider;
use alloy::rpc::types::BlockNumberOrTag;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;

/// Blocks below where the last pass stopped that every pass reads again. Older
/// logs are kept from earlier passes.
const REORG_MARGIN: u64 = 256;

/// Reads canonical, bounded log ranges at a fixed chain point. A pass reads the
/// reorg margin plus the blocks mined since the last pass, or since an interrupted
/// pass got cut short, so a reorg within the margin replaces terminal observations.
/// A deeper reorg goes unseen while this reader keeps its earlier logs.
pub struct EvmRpcReader {
    provider: RootProvider,
    chain_id: u64,
    max_blocks: u64,
    scans: std::sync::Mutex<std::collections::HashMap<String, Scan>>,
}
#[derive(Clone, Default)]
struct Scan {
    next: u64,
    logs: Vec<LifecycleLog>,
}
impl EvmRpcReader {
    pub async fn connect(url: url::Url, chain_id: u64, max_blocks_per_query: u64) -> Result<Self> {
        if max_blocks_per_query == 0 || max_blocks_per_query > 10000 {
            return Err(invalid("invalid EVM log range bound"));
        }
        let provider = crate::evm::http_provider(url);
        if provider.get_chain_id().await.map_err(transport)? != chain_id {
            return Err(invalid("EVM observation RPC chain mismatch"));
        }
        Ok(Self {
            provider,
            chain_id,
            max_blocks: max_blocks_per_query,
            scans: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }
    fn scan(&self, key: &str) -> Option<Scan> {
        self.scans
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }
    fn set_scan(&self, key: &str, scan: Scan) {
        self.scans
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string(), scan);
    }
}
fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}
impl ContractReader for EvmRpcReader {
    fn can_observe(&self, reference: &ContractRef) -> bool {
        matches!(reference,ContractRef::Evm {htlc,..} if htlc.chain_id==self.chain_id)
    }
    fn observe<'a>(&'a self, reference: &'a ContractRef) -> ObservationFuture<'a, Observation> {
        Box::pin(async move {
            let ContractRef::Evm {
                htlc,
                from_block,
                confirmations,
            } = reference
            else {
                return Err(invalid("EVM reader received another ledger"));
            };
            if !self.can_observe(reference) {
                return Err(invalid("EVM observer chain mismatch"));
            }
            let latest = self
                .provider
                .get_block_by_number(BlockNumberOrTag::Latest)
                .await
                .map_err(transport)?
                .ok_or_else(|| invalid("latest EVM block unavailable"))?;
            let point = ChainPoint {
                height: latest.header.number,
                hash: latest.header.hash.to_string(),
                timestamp: latest.header.timestamp,
            };
            let key = reference.key()?;
            let mut block_hashes = std::collections::HashMap::new();
            let mut scan = self.scan(&key).unwrap_or_default();
            // Logs within REORG_MARGIN of where the last pass stopped are read
            // again so a reorg replaces them.
            let mut from = scan
                .next
                .min(point.height.saturating_add(1))
                .saturating_sub(REORG_MARGIN)
                .max(*from_block);
            scan.logs
                .retain(|(height, ..)| *height < from && *height >= *from_block);
            while from <= point.height {
                let to = from.saturating_add(self.max_blocks - 1).min(point.height);
                let filter = Filter::new()
                    .address(htlc.address)
                    .topic1(htlc.hash_lock)
                    .from_block(from)
                    .to_block(to);
                for log in self.provider.get_logs(&filter).await.map_err(transport)? {
                    if log.removed || log.address() != htlc.address {
                        return Err(invalid("non-canonical EVM log"));
                    }
                    let height = log
                        .block_number
                        .ok_or_else(|| invalid("unmined EVM log in confirmed range"))?;
                    if height < from || height > to {
                        return Err(invalid("EVM log outside requested range"));
                    }
                    let canonical_hash = if let Some(hash) = block_hashes.get(&height) {
                        *hash
                    } else {
                        let block = self
                            .provider
                            .get_block_by_number(BlockNumberOrTag::Number(height))
                            .await
                            .map_err(transport)?
                            .ok_or_else(|| chain_changed("EVM log block disappeared"))?;
                        block_hashes.insert(height, block.header.hash);
                        block.header.hash
                    };
                    if log.block_hash != Some(canonical_hash) {
                        return Err(chain_changed("EVM log belongs to an orphaned block"));
                    }
                    scan.logs.push((
                        height,
                        log.log_index
                            .ok_or_else(|| invalid("EVM log index missing"))?,
                        log.topics().to_vec(),
                        log.data().data.to_vec(),
                    ));
                }
                from = to + 1;
                scan.next = from;
                self.set_scan(&key, scan.clone());
            }
            let canonical = self
                .provider
                .get_block_by_number(BlockNumberOrTag::Number(point.height))
                .await
                .map_err(transport)?
                .ok_or_else(|| chain_changed("observed EVM block disappeared"))?;
            if canonical.header.hash != latest.header.hash {
                return Err(chain_changed("EVM chain changed during observation"));
            }
            classify(htlc, *confirmations, point, scan.logs)
        })
    }
}
type LifecycleLog = (u64, u64, Vec<B256>, Vec<u8>);
pub(super) fn classify(
    expected: &EvmHtlc,
    confirmations: u32,
    point: ChainPoint,
    mut logs: Vec<LifecycleLog>,
) -> Result<Observation> {
    logs.sort_by_key(|(height, index, ..)| (*height, *index));
    let exact = expected.key();
    let has_exact = logs.iter().any(|(_, _, topics, data)| {
        SwapCreated::decode_raw_log_validate(topics.iter().copied(), data)
            .is_ok_and(|event| event.key == exact)
    });
    let mut funded = None;
    let mut funded_meets_terms = false;
    let mut state = HtlcState::Absent;
    let mut preimage = None;
    for (height, _, topics, data) in logs {
        let confirmed = point.height.saturating_sub(height) + 1 >= u64::from(confirmations);
        if let Ok(event) = SwapCreated::decode_raw_log_validate(topics.iter().copied(), &data) {
            if event.preimageHash != expected.hash_lock
                || event.refundAddress != expected.sender
                || event.claimAddress != expected.claimer
                || event.timelock != U256::from(expected.timelock)
            {
                continue;
            }
            let mut actual = expected.clone();
            actual.amount = event.amount;
            actual.token = event.token;
            if event.key != actual.key() {
                return Err(invalid("EVM funding event has an invalid swap key"));
            }
            let meets_terms = event.token == expected.token
                && event.amount >= expected.amount
                && !event.amount.is_zero();
            // Anyone can lock under this hash naming the expected refund address. The
            // contract rejects a second lock with the exact expected tuple, so that
            // lock wins; otherwise the earliest lock meeting the terms counts.
            if has_exact {
                if actual.key() != exact {
                    continue;
                }
            } else if funded
                .as_ref()
                .is_some_and(|previous: &EvmHtlc| previous.key() != actual.key())
                && (funded_meets_terms || !meets_terms)
            {
                continue;
            }
            funded_meets_terms = meets_terms;
            state = if !meets_terms {
                HtlcState::Invalid
            } else if confirmed {
                HtlcState::Confirmed
            } else {
                HtlcState::Mempool
            };
            funded = Some(actual);
        } else if let Some(funded) = &funded {
            if let Ok(event) = SwapRedeemed::decode_raw_log_validate(topics.iter().copied(), &data)
            {
                if event.preimageHash == expected.hash_lock && event.key == funded.key() {
                    preimage =
                        HashLock::Sha256(expected.hash_lock.0).preimage(event.preimage.as_slice());
                    if preimage.is_none() {
                        return Err(invalid("EVM redemption preimage mismatch"));
                    }
                    state = if confirmed {
                        HtlcState::SpentClaim
                    } else {
                        HtlcState::SpentUnknown
                    };
                }
            } else if let Ok(event) =
                SwapRefunded::decode_raw_log_validate(topics.iter().copied(), &data)
                && event.preimageHash == expected.hash_lock
                && event.key == funded.key()
            {
                state = if confirmed {
                    HtlcState::SpentRefund
                } else {
                    HtlcState::SpentUnknown
                };
            }
        }
    }
    let amount = funded.map_or(U256::ZERO, |htlc| htlc.amount);
    Ok(Observation {
        state,
        point: Some(point),
        funded_amount: amount.to_string(),
        remaining_amount: if matches!(
            state,
            HtlcState::SpentClaim | HtlcState::SpentRefund | HtlcState::SpentUnknown
        ) {
            "0".into()
        } else {
            amount.to_string()
        },
        preimage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evm::Address;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    fn htlc() -> EvmHtlc {
        EvmHtlc {
            chain_id: 42161,
            address: Address::repeat_byte(1),
            hash_lock: sha256::Hash::hash(&[42; 32]).to_byte_array().into(),
            amount: U256::from(10000),
            token: Address::repeat_byte(2),
            sender: Address::repeat_byte(3),
            claimer: Address::repeat_byte(4),
            timelock: 2_000_000_000,
        }
    }
    fn event(event: impl SolEvent, height: u64, index: u64) -> LifecycleLog {
        let data = event.encode_log_data();
        (height, index, data.topics().to_vec(), data.data.to_vec())
    }
    fn created(h: &EvmHtlc) -> LifecycleLog {
        event(
            SwapCreated {
                preimageHash: h.hash_lock,
                refundAddress: h.sender,
                claimAddress: h.claimer,
                token: h.token,
                amount: h.amount,
                timelock: U256::from(h.timelock),
                key: h.key(),
            },
            99,
            0,
        )
    }
    fn point() -> ChainPoint {
        ChainPoint {
            height: 100,
            hash: B256::repeat_byte(9).to_string(),
            timestamp: 1_900_000_000,
        }
    }
    #[test]
    fn evm_events_bind_the_full_tuple_amount_preimage_and_confirmation_depth() {
        let h = htlc();
        assert_eq!(
            classify(&h, 2, point(), vec![created(&h)]).unwrap().state,
            HtlcState::Confirmed
        );
        assert_eq!(
            classify(&h, 3, point(), vec![created(&h)]).unwrap().state,
            HtlcState::Mempool
        );
        let mut short = h.clone();
        short.amount = U256::from(9999);
        assert_eq!(
            classify(&h, 1, point(), vec![created(&short)])
                .unwrap()
                .state,
            HtlcState::Invalid
        );
        let mut other = h.clone();
        other.claimer = Address::repeat_byte(5);
        assert_eq!(
            classify(&h, 1, point(), vec![created(&other)])
                .unwrap()
                .state,
            HtlcState::Absent
        );
        let claim = event(
            SwapRedeemed {
                preimageHash: h.hash_lock,
                key: h.key(),
                preimage: B256::repeat_byte(42),
            },
            100,
            1,
        );
        let observed = classify(&h, 1, point(), vec![claim, created(&h)]).unwrap();
        assert_eq!(observed.state, HtlcState::SpentClaim);
        assert_eq!(observed.preimage.unwrap().expose(), &[42; 32]);
        let corrupt = event(
            SwapRedeemed {
                preimageHash: h.hash_lock,
                key: h.key(),
                preimage: B256::repeat_byte(41),
            },
            100,
            1,
        );
        assert!(classify(&h, 1, point(), vec![created(&h), corrupt]).is_err());
        let wrong_key = event(
            SwapRefunded {
                preimageHash: h.hash_lock,
                key: other.key(),
            },
            100,
            1,
        );
        assert_eq!(
            classify(&h, 1, point(), vec![created(&h), wrong_key])
                .unwrap()
                .state,
            HtlcState::Confirmed
        );
        let reference = ContractRef::Evm {
            htlc: h,
            from_block: 98,
            confirmations: 1,
        };
        let other = ContractRef::Evm {
            htlc: other,
            from_block: 98,
            confirmations: 1,
        };
        assert_ne!(reference.key().unwrap(), other.key().unwrap());
    }
    #[tokio::test]
    async fn rpc_observer_rejects_orphaned_logs_even_when_the_tip_is_stable() {
        let server = MockServer::start().await;
        let h = htlc();
        let contract = h.address;
        let creation = created(&h);
        Mock::given(method("POST")).respond_with(move |request:&wiremock::Request| {
            let body:serde_json::Value=request.body_json().unwrap();
            let result=match body["method"].as_str().unwrap() {
                "eth_chainId"=>json!("0xa4b1"),
                "eth_getBlockByNumber"=>{
                    let number=if body["params"][0]=="latest" {"0x64"} else {body["params"][0].as_str().unwrap()};
                    json!({"hash":B256::repeat_byte(9),"parentHash":B256::ZERO,"sha3Uncles":B256::ZERO,"miner":Address::ZERO,"stateRoot":B256::ZERO,"transactionsRoot":B256::ZERO,"receiptsRoot":B256::ZERO,"logsBloom":format!("0x{}","00".repeat(256)),"difficulty":"0x0","number":number,"gasLimit":"0x1000000","gasUsed":"0x0","timestamp":"0x713fb300","extraData":"0x","mixHash":B256::ZERO,"nonce":"0x0000000000000000","transactions":[],"uncles":[]})
                },
                "eth_getLogs"=>json!([{"address":contract,"topics":creation.2,"data":format!("0x{}",hex::encode(&creation.3)),"blockNumber":"0x63","blockHash":B256::repeat_byte(8),"transactionHash":B256::repeat_byte(7),"transactionIndex":"0x0","logIndex":"0x0","removed":false}]),
                method=>panic!("unexpected {method}"),
            };
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
        }).mount(&server).await;
        let reader = EvmRpcReader::connect(server.uri().parse().unwrap(), 42161, 100)
            .await
            .unwrap();
        let error = reader
            .observe(&ContractRef::Evm {
                htlc: h,
                from_block: 98,
                confirmations: 1,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Transport(message) if message.contains("orphaned")),
            "{error:?}"
        );
    }
    #[tokio::test]
    async fn a_chain_that_got_shorter_during_the_read_is_a_transport_error() {
        let h = htlc();
        let contract = h.address;
        let creation = created(&h);
        let log = json!([{"address":contract,"topics":creation.2,"data":format!("0x{}",hex::encode(&creation.3)),"blockNumber":"0x63","blockHash":B256::repeat_byte(8),"transactionHash":B256::repeat_byte(7),"transactionIndex":"0x0","logIndex":"0x0","removed":false}]);
        // Only the tip read first exists: the log's block, or the tip itself when read
        // again by number, is gone.
        for (logs, message) in [
            (log, "EVM log block disappeared"),
            (json!([]), "observed EVM block disappeared"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(move |request:&wiremock::Request| {
                let body:serde_json::Value=request.body_json().unwrap();
                let result=match body["method"].as_str().unwrap() {
                    "eth_chainId"=>json!("0xa4b1"),
                    "eth_getBlockByNumber" if body["params"][0]=="latest"=>json!({"hash":B256::repeat_byte(9),"parentHash":B256::ZERO,"sha3Uncles":B256::ZERO,"miner":Address::ZERO,"stateRoot":B256::ZERO,"transactionsRoot":B256::ZERO,"receiptsRoot":B256::ZERO,"logsBloom":format!("0x{}","00".repeat(256)),"difficulty":"0x0","number":"0x64","gasLimit":"0x1000000","gasUsed":"0x0","timestamp":"0x713fb300","extraData":"0x","mixHash":B256::ZERO,"nonce":"0x0000000000000000","transactions":[],"uncles":[]}),
                    "eth_getBlockByNumber"=>serde_json::Value::Null,
                    "eth_getLogs"=>logs.clone(),
                    method=>panic!("unexpected {method}"),
                };
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
            }).mount(&server).await;
            let reader = EvmRpcReader::connect(server.uri().parse().unwrap(), 42161, 100)
                .await
                .unwrap();
            let error = reader
                .observe(&ContractRef::Evm {
                    htlc: h.clone(),
                    from_block: 98,
                    confirmations: 1,
                })
                .await
                .unwrap_err();
            assert!(
                matches!(&error, Error::Transport(text) if text.contains(message)),
                "{error:?}"
            );
        }
    }
    #[tokio::test]
    async fn a_tip_replaced_during_the_read_is_a_transport_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(move |request:&wiremock::Request| {
            let body:serde_json::Value=request.body_json().unwrap();
            let result=match body["method"].as_str().unwrap() {
                "eth_chainId"=>json!("0xa4b1"),
                "eth_getBlockByNumber"=>{
                    // The tip read first is no longer canonical by the time the reader
                    // checks it again.
                    let hash=if body["params"][0]=="latest" {B256::repeat_byte(9)} else {B256::repeat_byte(10)};
                    json!({"hash":hash,"parentHash":B256::ZERO,"sha3Uncles":B256::ZERO,"miner":Address::ZERO,"stateRoot":B256::ZERO,"transactionsRoot":B256::ZERO,"receiptsRoot":B256::ZERO,"logsBloom":format!("0x{}","00".repeat(256)),"difficulty":"0x0","number":"0x64","gasLimit":"0x1000000","gasUsed":"0x0","timestamp":"0x713fb300","extraData":"0x","mixHash":B256::ZERO,"nonce":"0x0000000000000000","transactions":[],"uncles":[]})
                },
                "eth_getLogs"=>json!([]),
                method=>panic!("unexpected {method}"),
            };
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
        }).mount(&server).await;
        let reader = EvmRpcReader::connect(server.uri().parse().unwrap(), 42161, 100)
            .await
            .unwrap();
        let error = reader
            .observe(&ContractRef::Evm {
                htlc: htlc(),
                from_block: 98,
                confirmations: 1,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Transport(message) if message.contains("EVM chain changed")),
            "{error:?}"
        );
    }
    #[test]
    fn exact_lock_wins_over_an_earlier_decoy_meeting_the_terms() {
        let h = htlc();
        let mut decoy = h.clone();
        decoy.amount = h.amount + U256::from(1);
        let mut decoy_log = created(&decoy);
        decoy_log.0 = 98;
        let claim = event(
            SwapRedeemed {
                preimageHash: h.hash_lock,
                key: h.key(),
                preimage: B256::repeat_byte(42),
            },
            100,
            1,
        );
        let observed = classify(&h, 1, point(), vec![decoy_log, created(&h), claim]).unwrap();
        assert_eq!(observed.state, HtlcState::SpentClaim);
        assert_eq!(observed.preimage.unwrap().expose(), &[42; 32]);
    }
    #[test]
    fn valid_lock_replaces_a_short_lock_even_after_it_was_refunded() {
        let h = htlc();
        let mut short = h.clone();
        short.amount = U256::from(9999);
        let mut short_log = created(&short);
        short_log.0 = 97;
        let refund = event(
            SwapRefunded {
                preimageHash: h.hash_lock,
                key: short.key(),
            },
            98,
            0,
        );
        let mut over = h.clone();
        over.amount = U256::from(10001);
        let observed = classify(&h, 1, point(), vec![short_log, refund, created(&over)]).unwrap();
        assert_eq!(observed.state, HtlcState::Confirmed);
        assert_eq!(observed.funded_amount, "10001");
    }
    #[tokio::test]
    async fn second_pass_resumes_at_the_reorg_margin() {
        use std::sync::Arc;
        use std::sync::Mutex;
        use std::sync::atomic::AtomicU64;
        use std::sync::atomic::Ordering;
        let server = MockServer::start().await;
        let h = htlc();
        let contract = h.address;
        let mut creation = created(&h);
        creation.0 = 100;
        let claim = event(
            SwapRedeemed {
                preimageHash: h.hash_lock,
                key: h.key(),
                preimage: B256::repeat_byte(42),
            },
            9745,
            0,
        );
        let chain = [creation, claim];
        let tip = Arc::new(AtomicU64::new(10000));
        let requested = Arc::new(Mutex::new(Vec::new()));
        let (rpc_tip, rpc_requested) = (tip.clone(), requested.clone());
        let hex_number = |value: &serde_json::Value| {
            u64::from_str_radix(value.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
        };
        Mock::given(method("POST")).respond_with(move |request:&wiremock::Request| {
            let body:serde_json::Value=request.body_json().unwrap();
            let result=match body["method"].as_str().unwrap() {
                "eth_chainId"=>json!("0xa4b1"),
                "eth_getBlockByNumber"=>{
                    let number=if body["params"][0]=="latest" {format!("0x{:x}",rpc_tip.load(Ordering::SeqCst))} else {body["params"][0].as_str().unwrap().to_string()};
                    json!({"hash":B256::repeat_byte(9),"parentHash":B256::ZERO,"sha3Uncles":B256::ZERO,"miner":Address::ZERO,"stateRoot":B256::ZERO,"transactionsRoot":B256::ZERO,"receiptsRoot":B256::ZERO,"logsBloom":format!("0x{}","00".repeat(256)),"difficulty":"0x0","number":number,"gasLimit":"0x1000000","gasUsed":"0x0","timestamp":"0x713fb300","extraData":"0x","mixHash":B256::ZERO,"nonce":"0x0000000000000000","transactions":[],"uncles":[]})
                },
                "eth_getLogs"=>{
                    let from=hex_number(&body["params"][0]["fromBlock"]);
                    let to=hex_number(&body["params"][0]["toBlock"]);
                    rpc_requested.lock().unwrap().push(from);
                    json!(chain.iter().filter(|log| (from..=to).contains(&log.0)).map(|log| json!({"address":contract,"topics":log.2,"data":format!("0x{}",hex::encode(&log.3)),"blockNumber":format!("0x{:x}",log.0),"blockHash":B256::repeat_byte(9),"transactionHash":B256::repeat_byte(7),"transactionIndex":"0x0","logIndex":format!("0x{:x}",log.1),"removed":false})).collect::<Vec<_>>())
                },
                method=>panic!("unexpected {method}"),
            };
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
        }).mount(&server).await;
        let reader = EvmRpcReader::connect(server.uri().parse().unwrap(), 42161, 1000)
            .await
            .unwrap();
        let reference = ContractRef::Evm {
            htlc: h,
            from_block: 0,
            confirmations: 1,
        };
        let first = reader.observe(&reference).await.unwrap();
        assert_eq!(first.state, HtlcState::SpentClaim);
        assert_eq!(requested.lock().unwrap().first(), Some(&0));
        requested.lock().unwrap().clear();
        tip.store(12000, Ordering::SeqCst);
        let second = reader.observe(&reference).await.unwrap();
        assert_eq!(second.state, HtlcState::SpentClaim);
        assert_eq!(second.funded_amount, "10000");
        let requested = requested.lock().unwrap().clone();
        assert!(!requested.is_empty());
        assert!(
            requested.iter().all(|from| *from >= 10001 - REORG_MARGIN),
            "{requested:?}"
        );
        let key = reference.key().unwrap();
        assert_eq!(reader.scan(&key).unwrap().logs.len(), 2);
    }
}
