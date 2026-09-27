use super::funding::matching_creation;
use super::funding::transaction;
use super::funding::word;
use super::htlc::Coordinator;
use super::htlc::EvmHtlc;
use super::htlc::Htlc;
use super::*;
use crate::Client;
use crate::SwapMetadata;
use crate::types::EvmHtlcKind;
use alloy::sol_types::SolCall;
use std::time::Duration;

alloy::sol! {
    // HTLCNative drops the asset word from its write ABIs only; its key, event and
    // `isActive` keep it as the zero address.
    interface NativeHtlc {
        function refund(bytes32 preimageHash, uint256 amount, address claimAddress, uint256 timelock) external;
    }
    interface NativeCoordinator {
        function refundTo(bytes32 preimageHash, uint256 amount, address claimAddress, uint256 timelock) external;
    }
}

#[derive(Clone, Debug)]
pub struct RefundOptions {
    pub htlc: Address,
    pub lock_token: Address,
    /// None for direct HTLC funding; Some for a coordinator-funded swap.
    pub coordinator: Option<Address>,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvmSubmission {
    pub id: String,
    pub tx_hash: String,
    pub message: String,
}

impl Client {
    /// Read the actual locked amount from the persisted funding transaction,
    /// checking the emitting contract, sender, token, lock, and timelock.
    pub async fn get_funded_evm_htlc(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: &RefundOptions,
    ) -> Result<EvmHtlc> {
        self.read_funded_evm_htlc(id, wallet, options, true, false)
            .await
    }

    pub(super) async fn read_funded_evm_htlc(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: &RefundOptions,
        require_depositor: bool,
        any_submitter: bool,
    ) -> Result<EvmHtlc> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let context = EvmContext::from_response(&stored.response)?;
        // Coordinator refundTo pays deposits[key] whoever calls it, so any funded
        // wallet may submit; HTLC refund pays msg.sender and stays owner-only.
        if any_submitter && options.coordinator.is_some() {
            if wallet.chain_id() != context.chain_id {
                return Err(invalid("wallet chain does not match swap"));
            }
        } else {
            context.validate_wallet(wallet)?;
        }
        if !context.is_source || context.htlc != options.htlc {
            return Err(invalid("refund requires the configured EVM-source HTLC"));
        }
        if (context.kind == EvmHtlcKind::Native) != options.lock_token.is_zero() {
            return Err(invalid("lock token does not match the swap's HTLC kind"));
        }
        let value =
            serde_json::to_value(&stored.response).map_err(|e| Error::Decode(e.to_string()))?;
        let hash = stored
            .metadata
            .evm_fund_txid
            .as_deref()
            .or_else(|| value["evm_fund_txid"].as_str())
            .ok_or_else(|| {
                invalid("funding transaction is unknown; refresh or recover this swap")
            })?;
        let receipt = wallet.wait_for_receipt(parse(hash)?).await?;
        if receipt.transaction_hash != parse::<B256>(hash)? {
            return Err(invalid("funding receipt transaction hash mismatch"));
        }
        if !receipt.success {
            return Err(invalid("funding transaction reverted"));
        }
        let sender = options.coordinator.unwrap_or(context.owner);
        let created = matching_creation(&receipt.logs, &context, sender, options.lock_token)?
            .ok_or_else(|| invalid("funding receipt does not contain this HTLC"))?;
        let htlc = EvmHtlc {
            chain_id: context.chain_id,
            address: context.htlc,
            hash_lock: context.hash,
            amount: created.amount,
            token: options.lock_token,
            sender,
            claimer: context.server,
            timelock: context.timelock,
        };
        if htlc.key() != created.key {
            return Err(invalid("funding event HTLC key mismatch"));
        }
        if let Some(coordinator) = options.coordinator.filter(|_| require_depositor) {
            let depositor = wallet
                .call(&transaction(
                    wallet,
                    coordinator,
                    Coordinator::depositsCall { key: htlc.key() }.abi_encode(),
                ))
                .await?;
            let bytes = word(&depositor)?.to_be_bytes::<32>();
            if bytes[..12] != [0u8; 12] || Address::from_slice(&bytes[12..]) != context.owner {
                return Err(invalid(
                    "coordinator depositor does not match this swap",
                ));
            }
        }
        Ok(htlc)
    }

    pub async fn refund_evm_with_wallet(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: RefundOptions,
    ) -> Result<EvmReceipt> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.evm_execution.lock().await;
            let metadata = self
                .get_stored_swap(id)?
                .ok_or_else(|| invalid("swap record missing"))?
                .metadata;
            let htlc = self
                .read_funded_evm_htlc(
                    id,
                    wallet,
                    &options,
                    metadata.evm_refund_txid.is_none(),
                    true,
                )
                .await?;
            if let Some(hash) = metadata.evm_refund_txid {
                let hash = parse::<B256>(&hash)?;
                let receipt = wallet.wait_for_receipt(hash).await?;
                if receipt.transaction_hash != hash {
                    return Err(invalid("refund receipt hash mismatch"));
                }
                htlc.verify_refund_receipt(&receipt)?;
                return Ok(receipt);
            }
            if metadata.evm_refund_started == Some(true) {
                return Err(invalid(
                    "refund submission unresolved; attach its transaction, or release evm_refund_started once nothing was sent",
                ));
            }
            if !htlc.is_active(wallet).await? {
                return Err(invalid("HTLC is no longer active"));
            }
            if wallet.block_timestamp().await? < htlc.timelock {
                return Err(invalid("refund timelock has not expired on chain"));
            }
            let (to, data) = match (options.coordinator, htlc.token.is_zero()) {
                (Some(coordinator), false) => (
                    coordinator,
                    Coordinator::refundToCall {
                        preimageHash: htlc.hash_lock,
                        amount: htlc.amount,
                        token: htlc.token,
                        claimAddress: htlc.claimer,
                        timelock: U256::from(htlc.timelock),
                    }
                    .abi_encode(),
                ),
                (Some(coordinator), true) => (
                    coordinator,
                    NativeCoordinator::refundToCall {
                        preimageHash: htlc.hash_lock,
                        amount: htlc.amount,
                        claimAddress: htlc.claimer,
                        timelock: U256::from(htlc.timelock),
                    }
                    .abi_encode(),
                ),
                (None, false) => (
                    htlc.address,
                    Htlc::refundCall {
                        preimageHash: htlc.hash_lock,
                        amount: htlc.amount,
                        token: htlc.token,
                        claimAddress: htlc.claimer,
                        timelock: U256::from(htlc.timelock),
                    }
                    .abi_encode(),
                ),
                (None, true) => (
                    htlc.address,
                    NativeHtlc::refundCall {
                        preimageHash: htlc.hash_lock,
                        amount: htlc.amount,
                        claimAddress: htlc.claimer,
                        timelock: U256::from(htlc.timelock),
                    }
                    .abi_encode(),
                ),
            };
            let tx = transaction(wallet, to, data);
            wallet.call(&tx).await?;
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_refund_started: Some(true),
                    ..Default::default()
                },
            )?;
            let hash = wallet.send_transaction(&tx).await?;
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_refund_txid: Some(hash.to_string()),
                    ..Default::default()
                },
            )?;
            let receipt = wallet.wait_for_receipt(hash).await?;
            if receipt.transaction_hash != hash {
                return Err(invalid("refund receipt hash mismatch"));
            }
            htlc.verify_refund_receipt(&receipt)?;
            Ok(receipt)
        })
        .await
        .map_err(|_| {
            Error::Timeout("EVM refund timed out; inspect its stored transaction".into())
        })?
    }
}
