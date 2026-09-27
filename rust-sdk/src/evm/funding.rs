use super::*;
use crate::Client;
use crate::SwapMetadata;
use crate::aa::abi::Call;
use crate::aa::abi::IERC20;
use crate::aa::abi::IHTLCCoordinator;
use crate::aa::abi::PermitTransferFrom;
use crate::aa::abi::TokenPermissions;
use crate::aa::permit2::PERMIT2_ADDRESS;
use crate::aa::permit2::PermitWitnessParams;
use crate::aa::permit2::permit2_digest;
use alloy::primitives::keccak256;
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolValue;
use rand::RngCore;
use reqwest::Method;
use serde_json::json;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// A funding reserved with no recorded hash: sent or not, only the caller can tell.
pub(crate) const UNRESOLVED_FUNDING: &str = "funding may already be submitted; attach its transaction, or release evm_fund_started once nothing was sent";

sol! {
    interface TokenAllowance {
        function allowance(address owner, address spender) external view returns (uint256);
    }
    event SwapCreated(bytes32 indexed preimageHash, address indexed refundAddress, address indexed claimAddress, address token, uint256 amount, uint256 timelock, bytes32 key);
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FundingCall {
    pub target: String,
    pub value: String,
    pub call_data: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Permit2FundingData {
    pub coordinator_address: String,
    pub permit2_address: String,
    pub source_token_address: String,
    pub source_amount: String,
    pub lock_token_address: String,
    pub preimage_hash: String,
    pub claim_address: String,
    pub timelock: u64,
    pub calls: Vec<FundingCall>,
    pub calls_hash: String,
}

/// Explicit contract and spending limits for an execution attempt.
#[derive(Clone, Debug)]
pub struct FundingOptions {
    pub coordinator: Address,
    pub htlc: Address,
    pub lock_token: Address,
    pub max_source_amount: U256,
    pub allowed_call_targets: Vec<Address>,
    pub timeout: Duration,
}

impl Permit2FundingData {
    pub(crate) fn decoded_calls(&self) -> Result<Vec<Call>> {
        self.calls
            .iter()
            .map(|c| {
                Ok(Call {
                    target: parse(&c.target)?,
                    value: decimal(&c.value)?,
                    callData: parse(&c.call_data)?,
                })
            })
            .collect()
    }
    pub(crate) fn validate(
        &self,
        context: &EvmContext,
        policy: &FundingOptions,
    ) -> Result<Vec<Call>> {
        if !context.is_source
            || context.htlc != policy.htlc
            || parse::<Address>(&self.permit2_address)? != PERMIT2_ADDRESS
            || parse::<Address>(&self.coordinator_address)? != policy.coordinator
            || parse::<Address>(&self.lock_token_address)? != policy.lock_token
            || parse::<Address>(&self.source_token_address)? != context.source_token
            || parse::<B256>(&self.preimage_hash)? != context.hash
            || parse::<Address>(&self.claim_address)? != context.server
            || self.timelock != context.timelock
        {
            return Err(invalid(
                "funding payload does not match the swap and configured contracts",
            ));
        }
        let amount = decimal(&self.source_amount)?;
        if amount.is_zero() || amount > policy.max_source_amount {
            return Err(invalid("funding amount exceeds the approved limit"));
        }
        let calls = self.decoded_calls()?;
        if calls
            .iter()
            .any(|c| !policy.allowed_call_targets.contains(&c.target) || !c.value.is_zero())
        {
            return Err(invalid(
                "funding contains an unapproved target or native-token transfer",
            ));
        }
        if keccak256(calls.abi_encode()) != parse::<B256>(&self.calls_hash)? {
            return Err(invalid("funding calls hash mismatch"));
        }
        Ok(calls)
    }

    pub fn typed_data(&self, chain_id: u64, nonce: U256, deadline: U256) -> Result<EvmTypedData> {
        let witness = PermitWitnessParams {
            source_token: parse(&self.source_token_address)?,
            source_amount: decimal(&self.source_amount)?,
            coordinator_address: parse(&self.coordinator_address)?,
            nonce,
            deadline,
            preimage_hash: parse(&self.preimage_hash)?,
            lock_token: parse(&self.lock_token_address)?,
            claim_address: parse(&self.claim_address)?,
            refund_address: parse(&self.coordinator_address)?,
            timelock: U256::from(self.timelock),
            calls_hash: parse(&self.calls_hash)?,
        };
        let digest = permit2_digest(witness, chain_id);
        Ok(EvmTypedData {
            digest,
            payload: json!({
                "domain":{"name":"Permit2","chainId":chain_id,"verifyingContract":PERMIT2_ADDRESS},
                "primaryType":"PermitWitnessTransferFrom",
                "types": {
                    "EIP712Domain":[{"name":"name","type":"string"},{"name":"chainId","type":"uint256"},{"name":"verifyingContract","type":"address"}],
                    "TokenPermissions":[{"name":"token","type":"address"},{"name":"amount","type":"uint256"}],
                    "ExecuteAndCreate":[{"name":"preimageHash","type":"bytes32"},{"name":"token","type":"address"},{"name":"claimAddress","type":"address"},{"name":"refundAddress","type":"address"},{"name":"timelock","type":"uint256"},{"name":"callsHash","type":"bytes32"}],
                    "PermitWitnessTransferFrom":[{"name":"permitted","type":"TokenPermissions"},{"name":"spender","type":"address"},{"name":"nonce","type":"uint256"},{"name":"deadline","type":"uint256"},{"name":"witness","type":"ExecuteAndCreate"}]
                },
                "message":{"permitted":{"token":self.source_token_address,"amount":self.source_amount},"spender":self.coordinator_address,"nonce":nonce.to_string(),"deadline":deadline.to_string(),"witness":{"preimageHash":self.preimage_hash,"token":self.lock_token_address,"claimAddress":self.claim_address,"refundAddress":self.coordinator_address,"timelock":self.timelock.to_string(),"callsHash":self.calls_hash}}
            }),
        })
    }
}

pub(crate) fn transaction(wallet: &dyn EvmWallet, to: Address, data: Vec<u8>) -> EvmTransaction {
    EvmTransaction {
        chain_id: wallet.chain_id(),
        from: wallet.address(),
        to,
        value: U256::ZERO,
        data: data.into(),
    }
}
pub(crate) fn word(bytes: &[u8]) -> Result<U256> {
    if bytes.len() != 32 {
        return Err(Error::Decode("expected one ABI word".into()));
    }
    Ok(U256::from_be_slice(bytes))
}
pub(crate) fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| invalid(e.to_string()))
}

impl Client {
    pub(crate) fn swap_action_url(&self, id: &str, suffix: &str) -> Result<url::Url> {
        let mut url = self.base_url.join("/swap/")?;
        url.path_segments_mut()
            .map_err(|_| invalid("invalid API URL"))?
            .pop_if_empty()
            .push(id)
            .extend(suffix.split('/'));
        Ok(url)
    }

    pub async fn get_permit2_funding_data(&self, id: &str) -> Result<Permit2FundingData> {
        let response = self
            .api_request(
                Method::GET,
                self.swap_action_url(id, "swap-and-lock-calldata-permit2")?,
            )
            .send()
            .await?;
        Ok(crate::client::check_status(response).await?.json().await?)
    }

    /// Fund through an external wallet, approving only the bounded source amount.
    /// Fresh quotes are revalidated after approval and before signing. The funding
    /// transaction is persisted before waiting for its receipt.
    pub async fn fund_swap_with_wallet(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: FundingOptions,
    ) -> Result<EvmReceipt> {
        let timeout = options.timeout;
        tokio::time::timeout(
            timeout,
            self.fund_swap_with_wallet_inner(id, wallet, options),
        )
        .await
        .map_err(|_| {
            Error::Timeout(
                "EVM funding timed out; inspect the stored funding transaction before retrying"
                    .into(),
            )
        })?
    }

    async fn fund_swap_with_wallet_inner(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: FundingOptions,
    ) -> Result<EvmReceipt> {
        let _execution = self.evm_execution.lock().await;
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("recover or create the swap before funding"))?;
        if stored.metadata.evm_fund_txid.is_some()
            || stored.metadata.evm_fund_userop_hash.is_some()
            || stored
                .metadata
                .bridge_operations
                .as_ref()
                .is_some_and(|operations| operations.contains_key("cctp_fund"))
        {
            return Err(invalid(
                "funding already submitted; inspect its receipt before another attempt",
            ));
        }
        if stored.metadata.evm_fund_started == Some(true) {
            return Err(invalid(UNRESOLVED_FUNDING));
        }
        let context = EvmContext::from_response(&stored.response)?;
        context.validate_wallet(wallet)?;
        if context.kind == crate::types::EvmHtlcKind::Native {
            return Err(invalid(
                "funding a native lock is not supported by this SDK yet",
            ));
        }
        super::verify_derived_hash_lock(self, &stored, &context)?;
        let funding = self.get_permit2_funding_data(id).await?;
        funding.validate(&context, &options)?;
        let amount = decimal(&funding.source_amount)?;
        let balance = wallet
            .call(&transaction(
                wallet,
                context.source_token,
                IERC20::balanceOfCall {
                    account: wallet.address(),
                }
                .abi_encode(),
            ))
            .await?;
        if word(&balance)? < amount {
            return Err(invalid("insufficient token balance"));
        }
        let allowance = wallet
            .call(&transaction(
                wallet,
                context.source_token,
                TokenAllowance::allowanceCall {
                    owner: wallet.address(),
                    spender: PERMIT2_ADDRESS,
                }
                .abi_encode(),
            ))
            .await?;
        if word(&allowance)? < amount {
            let approve = transaction(
                wallet,
                context.source_token,
                IERC20::approveCall {
                    spender: PERMIT2_ADDRESS,
                    amount: options.max_source_amount,
                }
                .abi_encode(),
            );
            let hash = wallet.send_transaction(&approve).await?;
            let receipt = wallet.wait_for_receipt(hash).await?;
            if receipt.transaction_hash != hash {
                return Err(invalid("funding receipt transaction hash mismatch"));
            }
            if !receipt.success {
                return Err(invalid("token approval reverted"));
            }
        }
        let fresh = self.get_permit2_funding_data(id).await?;
        let calls = fresh.validate(&context, &options)?;
        let amount = decimal(&fresh.source_amount)?;
        // An exact-payout quote can change while the approval confirms.
        let balance = wallet
            .call(&transaction(
                wallet,
                context.source_token,
                IERC20::balanceOfCall {
                    account: wallet.address(),
                }
                .abi_encode(),
            ))
            .await?;
        let allowance = wallet
            .call(&transaction(
                wallet,
                context.source_token,
                TokenAllowance::allowanceCall {
                    owner: wallet.address(),
                    spender: PERMIT2_ADDRESS,
                }
                .abi_encode(),
            ))
            .await?;
        if word(&balance)? < amount || word(&allowance)? < amount {
            return Err(invalid(
                "fresh funding quote exceeds available balance or allowance",
            ));
        }
        let mut nonce = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let nonce = U256::from_be_bytes(nonce);
        let deadline = U256::from(
            now()?
                .checked_add(1800)
                .ok_or_else(|| invalid("deadline overflow"))?,
        );
        let typed = fresh.typed_data(context.chain_id, nonce, deadline)?;
        let signature = wallet.sign_typed_data(&typed).await?;
        verify_signature(&signature, typed.digest, context.owner)?;
        let call = IHTLCCoordinator::executeAndCreateWithPermit2Call {
            calls,
            preimageHash: context.hash,
            token: options.lock_token,
            claimAddress: context.server,
            timelock: U256::from(context.timelock),
            depositor: context.owner,
            permit: PermitTransferFrom {
                permitted: TokenPermissions {
                    token: context.source_token,
                    amount,
                },
                nonce,
                deadline,
            },
            signature: signature.as_bytes().to_vec().into(),
        };
        let tx = transaction(wallet, options.coordinator, call.abi_encode());
        wallet.call(&tx).await?;
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_fund_started: Some(true),
                evm_coordinator_address: Some(options.coordinator.to_string()),
                ..Default::default()
            },
        )?;
        let hash = wallet.send_transaction(&tx).await?;
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_fund_txid: Some(hash.to_string()),
                evm_coordinator_address: Some(options.coordinator.to_string()),
                ..Default::default()
            },
        )?;
        let receipt = wallet.wait_for_receipt(hash).await?;
        if !receipt.success {
            return Err(invalid(format!(
                "funding transaction {} reverted",
                receipt.transaction_hash
            )));
        }
        let created = matching_creation(
            &receipt.logs,
            &context,
            options.coordinator,
            options.lock_token,
        )?;
        if created.is_none() {
            return Err(invalid(
                "receipt contains no matching HTLC creation; inspect the stored transaction before retrying",
            ));
        }
        Ok(receipt)
    }
}

pub(crate) fn matching_creation(
    logs: &[EvmLog],
    context: &EvmContext,
    sender: Address,
    token: Address,
) -> Result<Option<SwapCreated>> {
    use alloy::sol_types::SolEvent;
    let mut matching = None;
    for log in logs.iter().filter(|log| log.address == context.htlc) {
        let Ok(created) =
            SwapCreated::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
        else {
            continue;
        };
        if created.preimageHash == context.hash
            && created.refundAddress == sender
            && created.claimAddress == context.server
            && created.token == token
            && created.timelock == U256::from(context.timelock)
        {
            let htlc = crate::evm::htlc::EvmHtlc {
                chain_id: context.chain_id,
                address: context.htlc,
                hash_lock: context.hash,
                amount: created.amount,
                token,
                sender,
                claimer: context.server,
                timelock: context.timelock,
            };
            if created.amount.is_zero() || created.key != htlc.key() {
                return Err(invalid("funding event has invalid amount or HTLC key"));
            }
            if matching.is_some() {
                return Err(invalid("multiple matching HTLC creation events"));
            }
            matching = Some(created);
        }
    }
    Ok(matching)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemorySwapStorage;
    use crate::Signer;
    use crate::StoredSwap;
    use crate::SwapStorage;
    use alloy::sol_types::SolEvent;
    use alloy::sol_types::SolValue;
    use std::sync::Arc;
    use std::sync::Mutex;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;

    const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn data() -> (EvmContext, Permit2FundingData, FundingOptions) {
        let context = EvmContext {
            chain_id: 137,
            owner: Address::repeat_byte(1),
            server: Address::repeat_byte(2),
            htlc: Address::repeat_byte(3),
            hash: B256::repeat_byte(4),
            timelock: 1_900_000_000,
            source_token: Address::repeat_byte(5),
            is_source: true,
            kind: crate::types::EvmHtlcKind::Erc20,
        };
        let coordinator = Address::repeat_byte(6);
        let token = Address::repeat_byte(7);
        let calls: Vec<Call> = vec![];
        let funding = Permit2FundingData {
            coordinator_address: coordinator.to_string(),
            permit2_address: PERMIT2_ADDRESS.to_string(),
            source_token_address: context.source_token.to_string(),
            source_amount: "1000".into(),
            lock_token_address: token.to_string(),
            preimage_hash: context.hash.to_string(),
            claim_address: context.server.to_string(),
            timelock: context.timelock,
            calls: vec![],
            calls_hash: keccak256(calls.abi_encode()).to_string(),
        };
        let policy = FundingOptions {
            coordinator,
            htlc: context.htlc,
            lock_token: token,
            max_source_amount: U256::from(1000),
            allowed_call_targets: vec![],
            timeout: Duration::from_secs(10),
        };
        (context, funding, policy)
    }

    #[test]
    fn funding_rejects_substituted_contracts_amounts_hashes_and_calls() {
        let (context, funding, policy) = data();
        funding.validate(&context, &policy).unwrap();
        let mut altered = funding.clone();
        altered.calls_hash = B256::ZERO.to_string();
        assert!(altered.validate(&context, &policy).is_err());
        let mut altered = funding.clone();
        altered.claim_address = Address::ZERO.to_string();
        assert!(altered.validate(&context, &policy).is_err());
        let mut altered = funding.clone();
        altered.permit2_address = Address::ZERO.to_string();
        assert!(altered.validate(&context, &policy).is_err());
        let mut altered = funding.clone();
        altered.source_amount = "1001".into();
        assert!(altered.validate(&context, &policy).is_err());
        let mut altered = funding;
        altered.calls.push(FundingCall {
            target: Address::repeat_byte(9).to_string(),
            value: "0".into(),
            call_data: "0x".into(),
        });
        assert!(altered.validate(&context, &policy).is_err());
    }

    #[test]
    fn typed_data_json_hash_matches_the_solidity_digest() {
        let (context, funding, _) = data();
        let typed = funding
            .typed_data(context.chain_id, U256::from(42), U256::from(1_800_000_000))
            .unwrap();
        let decoded: alloy_dyn_abi::TypedData = serde_json::from_value(typed.payload).unwrap();
        assert_eq!(decoded.eip712_signing_hash().unwrap(), typed.digest);
    }

    struct Wallet {
        context: EvmContext,
        secret: [u8; 32],
        receipt: EvmReceipt,
        transactions: Mutex<Vec<EvmTransaction>>,
        fail_receipt: bool,
        fail_send: bool,
    }
    impl EvmWallet for Wallet {
        fn address(&self) -> Address {
            self.context.owner
        }
        fn chain_id(&self) -> u64 {
            self.context.chain_id
        }
        fn sign_typed_data<'a>(&'a self, data: &'a EvmTypedData) -> WalletFuture<'a, Signature> {
            Box::pin(async move { crate::aa::signing::sign_hash(&self.secret, data.digest) })
        }
        fn call<'a>(&'a self, _: &'a EvmTransaction) -> WalletFuture<'a, Bytes> {
            Box::pin(async { Ok(U256::from(1000).to_be_bytes::<32>().to_vec().into()) })
        }
        fn send_transaction<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, B256> {
            Box::pin(async move {
                self.transactions.lock().unwrap().push(tx.clone());
                if self.fail_send {
                    return Err(Error::Transport("broadcast response lost".into()));
                }
                Ok(self.receipt.transaction_hash)
            })
        }
        fn wait_for_receipt(&self, _: B256) -> WalletFuture<'_, EvmReceipt> {
            Box::pin(async {
                if self.fail_receipt {
                    Err(Error::Transport("RPC unavailable after broadcast".into()))
                } else {
                    Ok(self.receipt.clone())
                }
            })
        }
    }

    #[tokio::test]
    async fn funding_checks_receipt_and_persists_before_a_failed_wait() {
        for (fail_receipt, fail_send) in [(false, false), (true, false), (false, true)] {
            let server = MockServer::start().await;
            let (mut context, mut funding, policy) = data();
            let signer = Signer::from_mnemonic(MNEMONIC).unwrap();
            let key = signer.derive_evm_key(0).unwrap();
            context.owner = parse(&key.address).unwrap();
            context.hash = B256::from(signer.derive_swap_params(0).unwrap().hash_lock);
            funding.preimage_hash = context.hash.to_string();
            Mock::given(path("/swap/test/swap-and-lock-calldata-permit2"))
                .respond_with(ResponseTemplate::new(200).set_body_json(&funding))
                .expect(2)
                .mount(&server)
                .await;
            let mut fixture: Value =
                serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json"))
                    .unwrap();
            let swap = &mut fixture["evm_to_arkade"];
            swap["id"] = json!("test");
            swap["evm_chain_id"] = json!(context.chain_id);
            swap["client_evm_address"] = json!(context.owner);
            swap["server_evm_address"] = json!(context.server);
            swap["evm_htlc_address"] = json!(context.htlc);
            swap["hash_lock"] = json!(context.hash);
            swap["evm_refund_locktime"] = json!(context.timelock);
            swap["source_token"]["token_id"] = json!(context.source_token);
            let mut foreign = swap.clone();
            foreign["hash_lock"] = json!(B256::repeat_byte(1));
            let store = Arc::new(InMemorySwapStorage::new());
            store
                .store_swap(
                    StoredSwap::new(
                        0,
                        serde_json::from_value(foreign).unwrap(),
                        SwapMetadata::default(),
                    )
                    .unwrap(),
                )
                .unwrap();
            let client = Client::builder()
                .base_url(server.uri())
                .mnemonic(MNEMONIC)
                .storage(store.clone())
                .build()
                .unwrap();
            let mut created = SwapCreated {
                preimageHash: context.hash,
                refundAddress: policy.coordinator,
                claimAddress: context.server,
                token: policy.lock_token,
                amount: U256::from(500),
                timelock: U256::from(context.timelock),
                key: B256::repeat_byte(9),
            };
            created.key = crate::evm::htlc::EvmHtlc {
                chain_id: context.chain_id,
                address: context.htlc,
                hash_lock: context.hash,
                amount: created.amount,
                token: policy.lock_token,
                sender: policy.coordinator,
                claimer: context.server,
                timelock: context.timelock,
            }
            .key();
            let log = created.encode_log_data();
            let receipt = EvmReceipt {
                transaction_hash: B256::repeat_byte(8),
                success: true,
                block_number: 123,
                logs: vec![EvmLog {
                    address: context.htlc,
                    topics: log.topics().to_vec(),
                    data: log.data.clone(),
                }],
            };
            let wallet = Wallet {
                context,
                secret: key.secret_key,
                receipt,
                transactions: Mutex::new(vec![]),
                fail_receipt,
                fail_send,
            };
            let rejected = client
                .fund_swap_with_wallet("test", &wallet, policy.clone())
                .await;
            assert!(
                matches!(&rejected, Err(crate::Error::InvalidSwap(message)) if message.contains("seed-derived")),
                "{rejected:?}"
            );
            assert!(wallet.transactions.lock().unwrap().is_empty());
            store
                .store_swap(
                    StoredSwap::new(
                        0,
                        serde_json::from_value(swap.clone()).unwrap(),
                        SwapMetadata::default(),
                    )
                    .unwrap(),
                )
                .unwrap();
            let result = client
                .fund_swap_with_wallet("test", &wallet, policy.clone())
                .await;
            assert_eq!(result.is_err(), fail_receipt || fail_send);
            assert_eq!(wallet.transactions.lock().unwrap().len(), 1);
            let stored = store.get_swap("test").unwrap().unwrap();
            assert_eq!(
                stored.metadata.evm_fund_txid,
                (!fail_send).then(|| wallet.receipt.transaction_hash.to_string())
            );
            assert_eq!(stored.metadata.evm_fund_started, Some(true));
            if fail_send {
                client
                    .attach_evm_funding_transaction("test", wallet.receipt.transaction_hash)
                    .await
                    .unwrap();
            }
            // Prevent accidental resubmission after a mined transaction or an interrupted wait.
            assert!(
                client
                    .fund_swap_with_wallet("test", &wallet, policy)
                    .await
                    .is_err()
            );
            assert_eq!(wallet.transactions.lock().unwrap().len(), 1);
        }
    }
}
