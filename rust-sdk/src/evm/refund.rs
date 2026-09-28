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

const SERVER_FUNDING_HEDGE: Duration = Duration::from_secs(2);

/// A failed funding check: whether the receipt was unavailable (so the server may
/// know a different hash that mined), and why.
pub(super) type Unverified = (bool, Error);

/// Checks a mined funding receipt: success, the single matching HTLC creation by
/// `sender`, and with `depositor_at`, that the coordinator's deposit belongs to the
/// swap owner. Like the TypeScript client, a reverted receipt or one without a
/// matching creation counts as unavailable; a proof that fails does not.
pub(super) async fn verify_funding_receipt(
    wallet: &dyn EvmWallet,
    context: &EvmContext,
    receipt: &EvmReceipt,
    sender: Address,
    lock_token: Address,
    depositor_at: Option<Address>,
) -> std::result::Result<EvmHtlc, Unverified> {
    let unavailable = |error| (true, error);
    let rejected = |error| (false, error);
    if !receipt.success {
        return Err(unavailable(invalid("funding transaction reverted")));
    }
    let created = matching_creation(&receipt.logs, context, sender, lock_token)
        .map_err(rejected)?
        .ok_or_else(|| unavailable(invalid("funding receipt does not contain this HTLC")))?;
    let htlc = EvmHtlc {
        chain_id: context.chain_id,
        address: context.htlc,
        hash_lock: context.hash,
        amount: created.amount,
        token: lock_token,
        sender,
        claimer: context.server,
        timelock: context.timelock,
    };
    if htlc.key() != created.key {
        return Err(rejected(invalid("funding event HTLC key mismatch")));
    }
    if let Some(coordinator) = depositor_at {
        let depositor = wallet
            .call(&transaction(
                wallet,
                coordinator,
                Coordinator::depositsCall { key: htlc.key() }.abi_encode(),
            ))
            .await
            .map_err(rejected)?;
        let bytes = word(&depositor).map_err(rejected)?.to_be_bytes::<32>();
        if bytes[..12] != [0u8; 12] || Address::from_slice(&bytes[12..]) != context.owner {
            return Err(rejected(invalid(
                "coordinator depositor does not match this swap",
            )));
        }
    }
    Ok(htlc)
}

/// Whichever result verifies first; if neither does, the error from `first`.
pub(super) async fn first_verified<T>(
    first: impl std::future::Future<Output = std::result::Result<T, Unverified>>,
    second: impl std::future::Future<Output = std::result::Result<T, Unverified>>,
) -> std::result::Result<T, Unverified> {
    tokio::pin!(first, second);
    tokio::select! {
        result = &mut first => match result {
            Ok(found) => Ok(found),
            Err(error) => second.await.map_err(|_| error),
        },
        result = &mut second => match result {
            Ok(found) => Ok(found),
            Err(_) => first.await,
        },
    }
}

impl Client {
    /// Read the actual locked amount from the funding transaction, checking the
    /// emitting contract, sender, token, lock, and timelock. When a replaced funding
    /// mined under the server-reported hash instead of the recorded one, that hash is
    /// verified and persisted.
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
        let local = stored.metadata.evm_fund_txid.as_deref();
        // A replaced funding leaves a stale local hash while the server reports the
        // mined one. The server hash only identifies the transaction to verify.
        let server = value["evm_fund_txid"].as_str();
        let context = &context;
        let verify = |hash: String| async move {
            let unavailable = |error| (true, error);
            let expected = parse::<B256>(&hash).map_err(unavailable)?;
            let receipt = wallet
                .wait_for_receipt(expected)
                .await
                .map_err(unavailable)?;
            if receipt.transaction_hash != expected {
                return Err(unavailable(invalid(
                    "funding receipt transaction hash mismatch",
                )));
            }
            let htlc = verify_funding_receipt(
                wallet,
                context,
                &receipt,
                options.coordinator.unwrap_or(context.owner),
                options.lock_token,
                options.coordinator.filter(|_| require_depositor),
            )
            .await?;
            Ok((htlc, hash))
        };
        let reason = |(_, error): Unverified| error;
        // Receipt waits on a hash that never mined run until they time out, so
        // candidates are checked at once.
        let (htlc, hash) = match (server, local) {
            (None, None) => {
                return Err(invalid(
                    "funding transaction is unknown; refresh or recover this swap",
                ));
            }
            (Some(server), Some(local)) if !server.eq_ignore_ascii_case(local) => {
                first_verified(verify(server.to_owned()), verify(local.to_owned()))
                    .await
                    .map_err(reason)?
            }
            (Some(hash), _) => verify(hash.to_owned()).await.map_err(reason)?,
            (None, Some(local)) => {
                // A replaced funding never mines under the recorded hash, so its receipt
                // lookup fails or runs until it times out. Either way the server is
                // asked which funding mined, while the recorded one is still checked.
                let recorded = verify(local.to_owned());
                tokio::pin!(recorded);
                let early = tokio::select! {
                    result = &mut recorded => Some(result),
                    () = tokio::time::sleep(SERVER_FUNDING_HEDGE) => None,
                };
                let lookup = || async {
                    self.server_funding_hash(id)
                        .await
                        .filter(|server| !server.eq_ignore_ascii_case(local))
                };
                // An unavailable recorded receipt falls back to the server's hash.
                let fallback = |error: Error| async {
                    match lookup().await {
                        Some(server) => verify(server).await.map_err(|_| error),
                        None => Err(error),
                    }
                };
                match early {
                    Some(Ok(found)) => found,
                    Some(Err((true, error))) => fallback(error).await?,
                    Some(Err((false, error))) => return Err(error),
                    None => {
                        let lookup = lookup();
                        tokio::pin!(lookup);
                        tokio::select! {
                            result = &mut recorded => match result {
                                Ok(found) => found,
                                Err((true, error)) => match lookup.await {
                                    Some(server) => verify(server).await.map_err(|_| error)?,
                                    None => return Err(error),
                                },
                                Err((false, error)) => return Err(error),
                            },
                            server = &mut lookup => match server {
                                Some(server) => first_verified(recorded, verify(server))
                                    .await
                                    .map_err(reason)?,
                                None => recorded.await.map_err(reason)?,
                            },
                        }
                    }
                }
            }
        };
        if local.is_none_or(|local| !local.eq_ignore_ascii_case(&hash)) {
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_fund_txid: Some(hash),
                    ..Default::default()
                },
            )?;
        }
        Ok(htlc)
    }

    pub(super) async fn server_funding_hash(&self, id: &str) -> Option<String> {
        let response = self.get_swap_details(id).await.ok()?;
        serde_json::to_value(&response).ok()?["evm_fund_txid"]
            .as_str()
            .map(str::to_owned)
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
