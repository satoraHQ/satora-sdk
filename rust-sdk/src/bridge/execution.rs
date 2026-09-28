use super::cctp;
use super::invalid;
use super::usdt0;
use crate::Client;
use crate::Result;
use crate::SwapMetadata;
use crate::aa::UserOpConfig;
use crate::aa::abi::Call;
use crate::aa::abi::IERC20;
use crate::control::PollOptions;
use crate::evm::funding::transaction;
use crate::evm::funding::word;
use crate::evm::*;
use crate::storage::BridgeOperation;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolEvent;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

/// Wallet transactions or one atomic Kernel batch. Sponsored execution derives
/// the account's matching key from the SDK seed; external wallets use Wallet.
#[derive(Clone, Debug)]
pub enum BridgeSubmission<'a> {
    Wallet,
    UserOperation(&'a UserOpConfig),
}
#[derive(Clone, Debug)]
pub struct CctpMint<'a> {
    pub burn: &'a cctp::CctpBurn,
    pub destination: &'a CctpDestination,
    pub attestation: &'a cctp::CctpAttestation,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CctpDestination {
    pub chain_id: u64,
    pub domain: u32,
    pub transmitter: Address,
    pub usdc: Address,
}
impl CctpDestination {
    pub fn mainnet(chain_id: u64) -> Result<Self> {
        let chain = cctp::CctpChain::for_chain(chain_id)?;
        Ok(Self {
            chain_id,
            domain: chain.domain,
            transmitter: cctp::MESSAGE_TRANSMITTER,
            usdc: chain.usdc,
        })
    }
    pub(super) fn validate(&self, burn: &cctp::CctpBurn, wallet: &dyn EvmWallet) -> Result<()> {
        if wallet.chain_id() != self.chain_id
            || self.domain != burn.destination_domain
            || self.transmitter.is_zero()
            || self.usdc.is_zero()
            || (!burn.destination_caller.is_zero()
                && burn.destination_caller != wallet.address().into_word())
        {
            return Err(invalid("CCTP destination wallet or contracts mismatch"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeRecovery {
    pub chain_id: u64,
    pub token: Address,
    pub recipient: Address,
    pub amount: U256,
}

alloy::sol! {
    interface Token {
        function allowance(address owner, address spender) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
    }
    event Transfer(address indexed from, address indexed to, uint256 value);
}

fn check_receipt(receipt: &EvmReceipt, hash: B256) -> Result<()> {
    if receipt.transaction_hash != hash || !receipt.success {
        return Err(invalid(
            "bridge transaction failed or receipt hash mismatch",
        ));
    }
    Ok(())
}
fn validate_burn_receipt(receipt: &EvmReceipt, burn: &cctp::CctpBurn) -> Result<()> {
    let count = receipt
        .logs
        .iter()
        .filter(|log| log.address == burn.source_transmitter)
        .filter(|log| {
            cctp::MessageSent::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
                .is_ok_and(|event| burn.validate_message(&event.message).is_ok())
        })
        .count();
    if count != 1 {
        return Err(invalid(
            "receipt must contain exactly one approved CCTP burn",
        ));
    }
    Ok(())
}
pub(super) fn validate_oft_receipt(
    receipt: &EvmReceipt,
    transfer: &usdt0::OftTransfer,
) -> Result<B256> {
    let matching: Vec<_> = usdt0::oft_sent(receipt, transfer.oft)
        .filter(|event| {
            event.dstEid == transfer.destination_eid
                && event.fromAddress == transfer.sender
                && event.amountSentLD <= transfer.amount
                && event.amountReceivedLD >= transfer.minimum_received
                && event.amountReceivedLD <= event.amountSentLD
                && !event.guid.is_zero()
        })
        .collect();
    if matching.len() != 1 {
        return Err(invalid(
            "receipt must contain exactly one approved OFT send",
        ));
    }
    Ok(matching[0].guid)
}

impl Client {
    pub(super) fn bridge_operation(&self, id: &str, key: &str) -> Result<Option<BridgeOperation>> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        Ok(stored
            .metadata
            .bridge_operations
            .and_then(|mut map| map.remove(key)))
    }
    pub(super) fn save_bridge_operation(
        &self,
        id: &str,
        key: &str,
        operation: BridgeOperation,
    ) -> Result<()> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let mut operations = stored.metadata.bridge_operations.unwrap_or_default();
        operations.insert(key.to_string(), operation);
        self.update_swap_metadata(
            id,
            SwapMetadata {
                bridge_operations: Some(operations),
                ..Default::default()
            },
        )?;
        Ok(())
    }
    pub(super) async fn bridge_receipt(
        &self,
        id: &str,
        key: &str,
        intent: &Value,
        wallet: &dyn EvmWallet,
        mode: &BridgeSubmission<'_>,
    ) -> Result<Option<EvmReceipt>> {
        let Some(mut operation) = self.bridge_operation(id, key)? else {
            return Ok(None);
        };
        if operation.intent != *intent {
            return Err(invalid(
                "bridge intent differs from its persisted operation",
            ));
        }
        let mut verified = None;
        if operation.transaction_hash.is_none()
            && let (Some(hash), BridgeSubmission::UserOperation(config)) =
                (&operation.user_operation_hash, mode)
        {
            let op = crate::evm::parse(hash)?;
            let bundler = crate::aa::bundler::BundlerClient::new(
                config.aa.bundler_url.clone(),
                config.aa.bundler_casing,
            )?;
            let receipt = bundler
                .get_user_operation_receipt(op)
                .await?
                .ok_or_else(|| invalid("bridge UserOperation is still pending"))?;
            if !receipt.success {
                return Err(invalid(
                    "bridge UserOperation reverted; inspect before recovery",
                ));
            }
            let tx = receipt.receipt.transaction_hash;
            let mined = wallet.wait_for_receipt(tx).await?;
            crate::aa::orchestrate::verify_user_operation_receipt(
                &mined,
                config.entry_point,
                op,
                tx,
            )?;
            operation.transaction_hash = Some(tx.to_string());
            self.save_bridge_operation(id, key, operation.clone())?;
            verified = Some(mined);
        }
        let hash = crate::evm::parse(operation.transaction_hash.as_deref().ok_or_else(|| {
            invalid("bridge submission outcome unknown; reconcile its hash before retrying")
        })?)?;
        let receipt = match verified {
            Some(receipt) => receipt,
            None => wallet.wait_for_receipt(hash).await?,
        };
        check_receipt(&receipt, hash)?;
        Ok(Some(receipt))
    }
    /// Resume an interrupted broadcast whose response was lost. The next execution
    /// call validates the supplied receipt against the original burn/mint/transfer.
    /// This never marks an operation complete or authorizes a replacement transfer.
    pub async fn attach_bridge_transaction(
        &self,
        id: &str,
        operation: &str,
        hash: B256,
    ) -> Result<()> {
        let _guard = self.evm_execution.lock().await;
        if hash.is_zero() {
            return Err(invalid("invalid bridge transaction hash"));
        }
        let mut state = self
            .bridge_operation(id, operation)?
            .ok_or_else(|| invalid("no pending bridge intent"))?;
        if state.complete || state.transaction_hash.is_some() {
            return Err(invalid("bridge transaction already recorded"));
        }
        state.transaction_hash = Some(hash.to_string());
        self.save_bridge_operation(id, operation, state)
    }
    pub(super) fn complete_bridge(&self, id: &str, key: &str) -> Result<()> {
        let mut state = self
            .bridge_operation(id, key)?
            .ok_or_else(|| invalid("bridge operation missing"))?;
        state.complete = true;
        self.save_bridge_operation(id, key, state)
    }
    pub(super) async fn submit_bridge(
        &self,
        id: &str,
        key: &str,
        intent: Value,
        wallet: &dyn EvmWallet,
        mode: &BridgeSubmission<'_>,
        calls: Vec<Call>,
    ) -> Result<EvmReceipt> {
        let (last, approvals) = calls
            .split_last()
            .ok_or_else(|| invalid("empty bridge batch"))?;
        let mut state = BridgeOperation {
            intent,
            transaction_hash: None,
            user_operation_hash: None,
            complete: false,
        };
        match mode {
            BridgeSubmission::Wallet => {
                for call in approvals {
                    let tx = EvmTransaction {
                        chain_id: wallet.chain_id(),
                        from: wallet.address(),
                        to: call.target,
                        value: call.value,
                        data: call.callData.clone(),
                    };
                    wallet.call(&tx).await?;
                    let hash = wallet.send_transaction(&tx).await?;
                    check_receipt(&wallet.wait_for_receipt(hash).await?, hash)?;
                }
                let tx = EvmTransaction {
                    chain_id: wallet.chain_id(),
                    from: wallet.address(),
                    to: last.target,
                    value: last.value,
                    data: last.callData.clone(),
                };
                wallet.call(&tx).await?;
                self.save_bridge_operation(id, key, state.clone())?;
                let hash = wallet.send_transaction(&tx).await?;
                state.transaction_hash = Some(hash.to_string());
                self.save_bridge_operation(id, key, state)?;
                let receipt = wallet.wait_for_receipt(hash).await?;
                check_receipt(&receipt, hash)?;
                Ok(receipt)
            }
            BridgeSubmission::UserOperation(config) => {
                let stored = self
                    .get_stored_swap(id)?
                    .ok_or_else(|| invalid("swap record missing"))?;
                let account_key = self
                    .signer
                    .as_ref()
                    .ok_or_else(|| invalid("signer missing"))?
                    .evm_key_for_address(stored.key_index, &wallet.address().to_string())?;
                let receipt = crate::aa::orchestrate::publish_with_config(
                    account_key.secret_key,
                    wallet.address(),
                    wallet.chain_id(),
                    &calls,
                    config,
                    &|| self.save_bridge_operation(id, key, state.clone()),
                    &|hash| {
                        let mut submitted = state.clone();
                        submitted.user_operation_hash = Some(hash.to_string());
                        self.save_bridge_operation(id, key, submitted)
                    },
                )
                .await?;
                let hash = receipt.transaction_hash.ok_or_else(|| {
                    invalid("bridge UserOperation pending; resume with persisted hash")
                })?;
                state.user_operation_hash = Some(receipt.user_op_hash.to_string());
                state.transaction_hash = Some(hash.to_string());
                self.save_bridge_operation(id, key, state)?;
                let receipt = wallet.wait_for_receipt(hash).await?;
                check_receipt(&receipt, hash)?;
                Ok(receipt)
            }
        }
    }
    /// Operation key `cctp_burn`. Approval is bounded to the approved burn amount.
    pub async fn burn_cctp(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        burn: &cctp::CctpBurn,
        mode: BridgeSubmission<'_>,
        options: &PollOptions,
    ) -> Result<EvmReceipt> {
        options
            .run(async {
                let _guard = self.evm_execution.lock().await;
                burn.validate()?;
                if wallet.address() != burn.sender || wallet.chain_id() != burn.source_chain_id {
                    return Err(invalid("CCTP source wallet mismatch"));
                }
                let intent = json!(burn);
                let receipt = match self
                    .bridge_receipt(id, "cctp_burn", &intent, wallet, &mode)
                    .await?
                {
                    Some(receipt) => receipt,
                    None => {
                        let mut calls = approval_calls(
                            wallet,
                            burn.burn_token,
                            burn.source_messenger,
                            burn.amount,
                        )
                        .await?;
                        calls.push(Call {
                            target: burn.source_messenger,
                            value: U256::ZERO,
                            callData: burn.calldata()?,
                        });
                        self.submit_bridge(id, "cctp_burn", intent, wallet, &mode, calls)
                            .await?
                    }
                };
                validate_burn_receipt(&receipt, burn)?;
                self.complete_bridge(id, "cctp_burn")?;
                Ok(receipt)
            })
            .await
    }
    /// Operation key `cctp_mint`. Returns None when the nonce was already consumed
    /// (for example by Circle's forwarder). Attestation verification happens on chain.
    pub async fn mint_cctp(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        mint: CctpMint<'_>,
        mode: BridgeSubmission<'_>,
        options: &PollOptions,
    ) -> Result<Option<EvmReceipt>> {
        let CctpMint {
            burn,
            destination,
            attestation,
        } = mint;
        options
            .run(async {
                let _guard = self.evm_execution.lock().await;
                destination.validate(burn, wallet)?;
                let calldata = attestation.receive_calldata(burn)?;
                let message: Bytes = crate::evm::parse(&attestation.message)?;
                let decoded = burn.validate_message(&message)?;
                let intent = json!({"burn":burn,"destination":destination,"nonce":decoded.nonce});
                let existing = self
                    .bridge_receipt(id, "cctp_mint", &intent, wallet, &mode)
                    .await?;
                let receipt = match existing {
                    Some(receipt) => receipt,
                    None => {
                        let used = word(
                            &wallet
                                .call(&transaction(
                                    wallet,
                                    destination.transmitter,
                                    cctp::usedNoncesCall {
                                        nonce: decoded.nonce,
                                    }
                                    .abi_encode(),
                                ))
                                .await?,
                        )?;
                        if used == U256::ONE {
                            return Ok(None);
                        }
                        if !used.is_zero() {
                            return Err(invalid("invalid CCTP nonce state"));
                        }
                        self.submit_bridge(
                            id,
                            "cctp_mint",
                            intent,
                            wallet,
                            &mode,
                            vec![Call {
                                target: destination.transmitter,
                                value: U256::ZERO,
                                callData: calldata,
                            }],
                        )
                        .await?
                    }
                };
                let valid = receipt
                    .logs
                    .iter()
                    .filter(|log| log.address == destination.transmitter)
                    .any(|log| {
                        cctp::MessageReceived::decode_raw_log_validate(
                            log.topics.iter().copied(),
                            &log.data,
                        )
                        .is_ok_and(|event| {
                            event.nonce == decoded.nonce
                                && event.sourceDomain == burn.source_domain
                                && event.sender == burn.source_messenger.into_word()
                                && event.messageBody.as_ref() == &message[148..]
                                && event.finalityThresholdExecuted >= burn.min_finality
                        })
                    });
                if !valid {
                    return Err(invalid("receipt does not contain the approved CCTP mint"));
                }
                self.complete_bridge(id, "cctp_mint")?;
                Ok(Some(receipt))
            })
            .await
    }
    /// Operation key `usdt0_send`. Returns the verified source event's GUID for tracking.
    pub async fn send_usdt0(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        transfer: &usdt0::OftTransfer,
        mode: BridgeSubmission<'_>,
        options: &PollOptions,
    ) -> Result<(EvmReceipt, B256)> {
        options
            .run(async {
                let _guard = self.evm_execution.lock().await;
                transfer.params()?;
                if wallet.chain_id() != transfer.chain_id || wallet.address() != transfer.sender {
                    return Err(invalid("OFT wallet mismatch"));
                }
                let intent = json!(transfer);
                let receipt = match self
                    .bridge_receipt(id, "usdt0_send", &intent, wallet, &mode)
                    .await?
                {
                    Some(receipt) => receipt,
                    None => {
                        let fee = transfer.quote(wallet).await?;
                        let mut calls =
                            approval_calls(wallet, transfer.token, transfer.oft, transfer.amount)
                                .await?;
                        calls.push(Call {
                            target: transfer.oft,
                            value: fee,
                            callData: transfer.calldata(fee)?,
                        });
                        self.submit_bridge(id, "usdt0_send", intent, wallet, &mode, calls)
                            .await?
                    }
                };
                let guid = validate_oft_receipt(&receipt, transfer)?;
                self.complete_bridge(id, "usdt0_send")?;
                Ok((receipt, guid))
            })
            .await
    }
    /// Recover minted tokens to an explicit address with a bounded amount.
    /// Operation key `bridge_sweep`; replay verifies the stored transfer receipt.
    /// Once that sweep is complete, a different recovery starts a new transfer.
    pub async fn recover_bridge_balance(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        recovery: &BridgeRecovery,
        mode: BridgeSubmission<'_>,
        options: &PollOptions,
    ) -> Result<EvmReceipt> {
        options
            .run(async {
                let _guard = self.evm_execution.lock().await;
                if recovery.chain_id != wallet.chain_id()
                    || recovery.token.is_zero()
                    || recovery.recipient.is_zero()
                    || recovery.recipient == wallet.address()
                    || recovery.amount.is_zero()
                {
                    return Err(invalid("invalid bridge recovery"));
                }
                let intent = json!({"recovery":recovery,"sender":wallet.address()});
                let settled_elsewhere = self
                    .bridge_operation(id, "bridge_sweep")?
                    .is_some_and(|op| op.complete && op.intent != intent);
                let existing = if settled_elsewhere {
                    None
                } else {
                    self.bridge_receipt(id, "bridge_sweep", &intent, wallet, &mode)
                        .await?
                };
                let receipt = match existing {
                    Some(receipt) => receipt,
                    None => {
                        if token_balance(wallet, recovery.token).await? < recovery.amount {
                            return Err(invalid("insufficient bridge recovery balance"));
                        }
                        self.submit_bridge(
                            id,
                            "bridge_sweep",
                            intent,
                            wallet,
                            &mode,
                            vec![Call {
                                target: recovery.token,
                                value: U256::ZERO,
                                callData: Token::transferCall {
                                    to: recovery.recipient,
                                    amount: recovery.amount,
                                }
                                .abi_encode()
                                .into(),
                            }],
                        )
                        .await?
                    }
                };
                let valid = receipt
                    .logs
                    .iter()
                    .filter(|log| log.address == recovery.token)
                    .any(|log| {
                        Transfer::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
                            .is_ok_and(|event| {
                                event.from == wallet.address()
                                    && event.to == recovery.recipient
                                    && event.value == recovery.amount
                            })
                    });
                if !valid {
                    return Err(invalid(
                        "receipt does not contain the approved bridge recovery transfer",
                    ));
                }
                self.complete_bridge(id, "bridge_sweep")?;
                Ok(receipt)
            })
            .await
    }
}
async fn token_balance(wallet: &dyn EvmWallet, token: Address) -> Result<U256> {
    word(
        &wallet
            .call(&transaction(
                wallet,
                token,
                IERC20::balanceOfCall {
                    account: wallet.address(),
                }
                .abi_encode(),
            ))
            .await?,
    )
}
async fn approval_calls(
    wallet: &dyn EvmWallet,
    token: Address,
    spender: Address,
    amount: U256,
) -> Result<Vec<Call>> {
    if token_balance(wallet, token).await? < amount {
        return Err(invalid("insufficient bridge token balance"));
    }
    let allowance = word(
        &wallet
            .call(&transaction(
                wallet,
                token,
                Token::allowanceCall {
                    owner: wallet.address(),
                    spender,
                }
                .abi_encode(),
            ))
            .await?,
    )?;
    let mut calls = Vec::new();
    if allowance < amount {
        if !allowance.is_zero() {
            calls.push(Call {
                target: token,
                value: U256::ZERO,
                callData: IERC20::approveCall {
                    spender,
                    amount: U256::ZERO,
                }
                .abi_encode()
                .into(),
            });
        }
        calls.push(Call {
            target: token,
            value: U256::ZERO,
            callData: IERC20::approveCall { spender, amount }.abi_encode().into(),
        });
    }
    Ok(calls)
}
