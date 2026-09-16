use super::*;
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

/// Reads canonical, bounded log ranges at a fixed chain point. Re-reading is
/// intentional: terminal observations are not latched across reorgs.
pub struct EvmRpcReader {
    provider: RootProvider,
    chain_id: u64,
    max_blocks: u64,
}
impl EvmRpcReader {
    pub async fn connect(url: url::Url, chain_id: u64, max_blocks_per_query: u64) -> Result<Self> {
        if max_blocks_per_query == 0 || max_blocks_per_query > 10000 {
            return Err(invalid("invalid EVM log range bound"));
        }
        let provider = RootProvider::new_http(url);
        if provider.get_chain_id().await.map_err(transport)? != chain_id {
            return Err(invalid("EVM observation RPC chain mismatch"));
        }
        Ok(Self {
            provider,
            chain_id,
            max_blocks: max_blocks_per_query,
        })
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
            let mut logs = Vec::new();
            let mut block_hashes = std::collections::HashMap::new();
            let mut from = *from_block;
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
                            .ok_or_else(|| invalid("EVM log block disappeared"))?;
                        block_hashes.insert(height, block.header.hash);
                        block.header.hash
                    };
                    if log.block_hash != Some(canonical_hash) {
                        return Err(invalid("EVM log belongs to an orphaned block"));
                    }
                    logs.push((
                        height,
                        log.log_index
                            .ok_or_else(|| invalid("EVM log index missing"))?,
                        log.topics().to_vec(),
                        log.data().data.to_vec(),
                    ));
                }
                if to == point.height {
                    break;
                }
                from = to + 1;
            }
            let canonical = self
                .provider
                .get_block_by_number(BlockNumberOrTag::Number(point.height))
                .await
                .map_err(transport)?
                .ok_or_else(|| invalid("observed EVM block disappeared"))?;
            if canonical.header.hash != latest.header.hash {
                return Err(invalid("EVM chain changed during observation"));
            }
            classify(htlc, *confirmations, point, logs)
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
    let mut funded = None;
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
            if funded
                .as_ref()
                .is_some_and(|previous: &EvmHtlc| previous.key() != actual.key())
            {
                return Err(invalid("ambiguous EVM funding tuples"));
            }
            state = if event.token != expected.token
                || event.amount < expected.amount
                || event.amount.is_zero()
            {
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
        assert!(error.to_string().contains("orphaned"), "{error}");
    }
}
