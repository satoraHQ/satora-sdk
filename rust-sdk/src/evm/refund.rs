use super::funding::matching_creation;
use super::funding::transaction;
use super::funding::word;
use super::htlc::Coordinator;
use super::htlc::EvmHtlc;
use super::htlc::Htlc;
use super::*;
use crate::Client;
use crate::SwapMetadata;
use crate::client::check_status;
use crate::logging::sdk_log;
use crate::types::EvmHtlcKind;
use alloy::sol_types::SolCall;
use std::time::Duration;

alloy::sol! {
    // HTLCNative drops the asset word from its write ABIs only; its key, event and
    // `isActive` keep it as the zero address.
    interface NativeHtlc {
        function refund(bytes32 preimageHash, uint256 amount, address claimAddress, uint256 timelock) external;
        function refundBySig(bytes32 preimageHash, uint256 amount, uint256 timelock, uint8 v, bytes32 r, bytes32 s) external returns (address claimAddress);
        function rehash(bytes32 preimageHash, uint256 amount, uint256 timelock, bytes32 newPreimageHash, uint256 newTimelock, uint8 v, bytes32 r, bytes32 s) external returns (address claimAddress, bytes32 newKey);
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

/// `GET /api/swap/{id}/collab-refund-evm/params`: how the swap refunds early.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollabRefundEvmParams {
    pub htlc_address: String,
    pub htlc_version: i64,
    pub htlc_kind: crate::types::EvmHtlcKind,
    pub chain_id: u64,
    pub preimage_hash: String,
    pub amount: String,
    pub token: String,
    pub claim_address: String,
    pub timelock: u64,
    /// `direct`: the depositor sends `refundBySig` with the server's signature;
    /// `coordinator`: the depositor signs `CollabRefund` and the server submits;
    /// `unavailable`: the swap waits for its timelock.
    pub refund_path: String,
}

/// `POST /api/swap/{id}/collab-refund-evm/signature`: the server's `Refund`
/// signature over the lock's exact terms, valid only from `sender`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollabRefundEvmSignature {
    pub id: String,
    pub htlc_address: String,
    pub htlc_version: i64,
    pub htlc_kind: crate::types::EvmHtlcKind,
    pub chain_id: u64,
    pub preimage_hash: String,
    pub amount: String,
    pub token: String,
    pub sender: String,
    pub claim_address: String,
    pub timelock: u64,
    pub v: u8,
    pub r: String,
    pub s: String,
}

const SERVER_FUNDING_HEDGE: Duration = Duration::from_secs(2);

/// The refund hash still awaiting verification. Once its revert is seen it is
/// copied to `evm_refund_reverted`, because merge cannot clear `evm_refund_txid`.
pub(super) fn pending_refund(metadata: &SwapMetadata) -> Option<&String> {
    metadata
        .evm_refund_txid
        .as_ref()
        .filter(|hash| metadata.evm_refund_reverted.as_ref() != Some(*hash))
}

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
    let created = match matching_creation(&receipt.logs, context, sender, lock_token)
        .map_err(rejected)?
    {
        Some(created) => created,
        None => {
            // The server names the rehash tx as the funding tx once it has
            // confirmed a move; without the local record of that move the
            // lock's origin cannot be found from here.
            if rehashed_into(receipt, context).is_some() {
                return Err(rejected(invalid(
                    "the funding transaction moved this lock by a rehash this client has no record of; use the store that holds the swap's rehash history",
                )));
            }
            return Err(unavailable(invalid(
                "funding receipt does not contain this HTLC",
            )));
        }
    };
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
        let depositor = word(&depositor).map_err(rejected)?;
        if depositor.is_zero() {
            // The coordinator deletes a deposit once it refunds or claims its HTLC.
            return Err(rejected(invalid(
                "the coordinator holds no deposit for this HTLC: it was refunded or claimed; check the depositor's balance of the lock token",
            )));
        }
        let bytes = depositor.to_be_bytes::<32>();
        if bytes[..12] != [0u8; 12] || Address::from_slice(&bytes[12..]) != context.owner {
            return Err(rejected(invalid(
                "coordinator depositor does not match this swap",
            )));
        }
    }
    Ok(htlc)
}

/// The `newKey` of a `SwapRehashed` in `receipt` that moved a lock on the
/// swap's HTLC to its current hash.
pub(super) fn rehashed_into(receipt: &EvmReceipt, context: &EvmContext) -> Option<B256> {
    use alloy::sol_types::SolEvent;
    receipt
        .logs
        .iter()
        .filter(|log| log.address == context.htlc)
        .filter_map(|log| {
            super::htlc::SwapRehashed::decode_raw_log_validate(
                log.topics.iter().copied(),
                &log.data,
            )
            .ok()
        })
        .find(|event| event.newPreimageHash == context.hash)
        .map(|event| event.newKey)
}

/// The lock a seed recovery saved, checked against its rehash's receipt
/// again: the record must name the lock that rehash moved to.
async fn recovered_lock(
    wallet: &dyn EvmWallet,
    context: &EvmContext,
    recovered: &crate::storage::EvmRecoveredLock,
    lock_token: Address,
) -> Result<EvmHtlc> {
    let htlc = EvmHtlc {
        chain_id: context.chain_id,
        address: context.htlc,
        hash_lock: parse(&recovered.hash_lock)?,
        amount: decimal(&recovered.amount)?,
        token: parse(&recovered.token)?,
        sender: parse(&recovered.sender)?,
        claimer: parse(&recovered.claim_address)?,
        timelock: recovered.timelock,
    };
    if htlc.token != lock_token {
        return Err(invalid("the recovered lock holds another token"));
    }
    let txid = parse::<B256>(&recovered.txid)?;
    let receipt = wallet.wait_for_receipt(txid).await?;
    let moved = EvmContext {
        hash: htlc.hash_lock,
        ..context.clone()
    };
    if receipt.transaction_hash != txid
        || !receipt.success
        || rehashed_into(&receipt, &moved) != Some(htlc.key())
    {
        return Err(invalid(
            "the recovered lock is not the one its rehash transaction moved to",
        ));
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

    /// The HTLC an EVM to Arkade swap owner's own funding created, locking
    /// `lock_token`: the funding receipt, the coordinator as lock sender and the
    /// owner as the coordinator's depositor are all checked, so a lock someone
    /// else made under the same hash cannot stand in for it. A refresh can rewrite
    /// the stored response, so the hash lock, the owner and the coordinator must
    /// be what this wallet holds: its own hash lock, one of its own addresses, and
    /// the coordinator its own funding went through. Only that funding can make
    /// one of its addresses a depositor under its hash.
    pub(crate) async fn owner_funded_evm_htlc(
        &self,
        id: &str,
        reader: &dyn EvmWallet,
        lock_token: Address,
    ) -> Result<EvmHtlc> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let crate::types::GetSwapResponse::EvmToArkade(response) = &stored.response else {
            return Err(invalid("not an EVM to Arkade swap"));
        };
        let context = EvmContext::from_response(&stored.response)?;
        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| invalid("reading a swap's own EVM lock requires a signer"))?;
        if context.hash != B256::from(signer.derive_swap_params(stored.key_index)?.hash_lock) {
            return Err(invalid("the swap's EVM hash lock is not this wallet's"));
        }
        signer
            .evm_key_for_address(stored.key_index, &context.owner.to_string())
            .map_err(|_| invalid("the swap's EVM owner is not an address of this wallet"))?;
        let coordinator = parse(
            stored
                .metadata
                .evm_coordinator_address
                .as_deref()
                .ok_or_else(|| {
                    invalid("the coordinator this wallet funded the swap through is unknown")
                })?,
        )?;
        let options = RefundOptions {
            htlc: parse(&response.evm_htlc_address)?,
            lock_token,
            coordinator: Some(coordinator),
            timeout: RESUME_TIMEOUT,
        };
        self.read_funded_evm_htlc(id, reader, &options, true, true)
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
        let mut context = EvmContext::from_response(&stored.response)?;
        // The funding tx created the lock under the terms before any rehash this
        // client sent; the server's copy follows a move once it confirms it.
        let rehashes = stored.metadata.evm_rehashes.clone().unwrap_or_default();
        if let Some(origin) = rehashes.first() {
            context.hash = parse(&origin.from_hash_lock)?;
            context.timelock = origin.from_timelock;
        }
        let sender = context.lock_sender(options.coordinator);
        // Reading the lock needs only the right chain. Submitting does not: the
        // coordinator's refundTo pays deposits[key] whoever calls it, while the
        // HTLC's refund pays msg.sender, which `refund_evm` enforces at the send.
        if any_submitter {
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
        // A lock recovery verified stands on its own record, whatever the
        // server reports now; the rehashes this store sent since follow it.
        if let Some(recovered) = stored
            .metadata
            .evm_recovered_lock
            .as_ref()
            .filter(|_| context.depositor_is_sender())
        {
            return recovered_lock(wallet, &context, recovered, options.lock_token)
                .await?
                .after_rehashes(wallet, &rehashes)
                .await;
        }
        let value =
            serde_json::to_value(&stored.response).map_err(|e| Error::Decode(e.to_string()))?;
        let local = stored.metadata.evm_fund_txid.as_deref();
        // A replaced funding leaves a stale local hash while the server reports the
        // mined one. The server hash only identifies the transaction to verify.
        let server = value["evm_fund_txid"].as_str();
        // Recovered from the seed after the server promoted a rehash: the
        // server's funding tx is the move, and this store has no record of it.
        let unrecorded_move = rehashes.is_empty() && context.depositor_is_sender();
        let expected_sats = value["evm_expected_sats"].as_str();
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
            let verified = verify_funding_receipt(
                wallet,
                context,
                &receipt,
                sender,
                options.lock_token,
                options
                    .coordinator
                    .filter(|coordinator| require_depositor && *coordinator == sender),
            )
            .await;
            match (verified, rehashed_into(&receipt, context)) {
                (Err(_), Some(new_key)) if unrecorded_move && receipt.success => {
                    let htlc = self
                        .unrecorded_rehash(
                            id,
                            wallet,
                            context,
                            options.lock_token,
                            expected_sats,
                            new_key,
                        )
                        .await
                        .map_err(|error| (false, error))?;
                    Ok((htlc, hash, true))
                }
                (verified, _) => Ok((verified?, hash, false)),
            }
        };
        let reason = |(_, error): Unverified| error;
        // Receipt waits on a hash that never mined run until they time out, so
        // candidates are checked at once.
        let (htlc, hash, recovered) = match (server, local) {
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
                    self.server_funding_hash(id, "client.refundSwap.serverLookupFailed")
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
        // A verified recovered lock is saved, so later reads, a retry's
        // rehash and a submitted refund need no server terms again.
        let recovered = recovered.then(|| crate::storage::EvmRecoveredLock {
            txid: hash.clone(),
            hash_lock: htlc.hash_lock.to_string(),
            timelock: htlc.timelock,
            amount: htlc.amount.to_string(),
            token: htlc.token.to_string(),
            sender: htlc.sender.to_string(),
            claim_address: htlc.claimer.to_string(),
        });
        if recovered.is_some() || local.is_none_or(|local| !local.eq_ignore_ascii_case(&hash)) {
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_fund_txid: Some(hash),
                    evm_recovered_lock: recovered,
                    ..Default::default()
                },
            )?;
        }
        htlc.after_rehashes(wallet, &rehashes).await
    }

    /// The lock a rehash this store has no record of moved to the swap's
    /// current hash. Its terms come from the server, so they count only if
    /// they hash to the event's `newKey` with the depositor as sender, and
    /// that lock is active on chain. The quoted amount is tried first; the
    /// server's locked amount is asked for only when the quote does not match.
    async fn unrecorded_rehash(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        context: &EvmContext,
        lock_token: Address,
        expected_sats: Option<&str>,
        new_key: B256,
    ) -> Result<EvmHtlc> {
        let lock = |amount| EvmHtlc {
            chain_id: context.chain_id,
            address: context.htlc,
            hash_lock: context.hash,
            amount,
            token: lock_token,
            sender: context.owner,
            claimer: context.server,
            timelock: context.timelock,
        };
        let matches = |amount: Option<U256>| amount.map(lock).filter(|htlc| htlc.key() == new_key);
        let quoted = matches(expected_sats.and_then(|sats| decimal(sats).ok()));
        let htlc = match quoted {
            Some(htlc) => htlc,
            // The locked amount can differ from the quote; the server knows it.
            None => {
                let locked = match self.collab_refund_evm_params(id).await {
                    Ok(params) => decimal(&params.amount).ok(),
                    Err(error) => {
                        sdk_log!(
                            self.log.clone().swap_id(id),
                            Debug,
                            "client.refundSwap.serverTermsFailed",
                            "Server lookup of the lock terms failed",
                            "error" => error.to_string()
                        );
                        None
                    }
                };
                matches(locked).ok_or_else(|| {
                    invalid(
                        "the server's terms do not match the lock the funding transaction rehashed; use the store that holds the swap's rehash history",
                    )
                })?
            }
        };
        if !htlc.is_active(wallet).await? {
            return Err(invalid(
                "the lock the funding transaction rehashed is no longer active",
            ));
        }
        Ok(htlc)
    }

    async fn collab_refund_evm_params(&self, id: &str) -> Result<CollabRefundEvmParams> {
        Ok(check_status(
            self.api_request(
                reqwest::Method::GET,
                self.api_swap_url(id, "collab-refund-evm/params")?,
            )
            .send()
            .await?,
        )
        .await?
        .json::<CollabRefundEvmParams>()
        .await?)
    }

    pub(super) async fn server_funding_hash(
        &self,
        id: &str,
        event: &'static str,
    ) -> Option<String> {
        // Unchecked, as a refund must not depend on the server's current invoice.
        let response = match self.fetch_swap_response(id).await {
            Ok(response) => response,
            Err(error) => {
                sdk_log!(
                    self.log.clone().swap_id(id),
                    Debug,
                    event,
                    "Server lookup of the funding transaction failed",
                    "error" => error.to_string()
                );
                return None;
            }
        };
        serde_json::to_value(&response).ok()?["evm_fund_txid"]
            .as_str()
            .map(str::to_owned)
    }

    /// Refund the swap's funded EVM HTLC from `wallet`. A refund whose hash is
    /// recorded is verified on retry, never sent a second time, and one that
    /// mined and reverted refunded nothing: it is released with
    /// [`Error::InvalidSwap`], and the next call sends a new one while the HTLC
    /// is active.
    pub async fn refund_evm_with_wallet(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: RefundOptions,
    ) -> Result<EvmReceipt> {
        self.refund_evm(id, wallet, options, false).await
    }

    /// With `depositor`, the swap's own depositor pays the gas: the refund is
    /// refused before anything is reserved when it cannot, and a refund reserved
    /// without a recorded hash is sent again while the HTLC is active, since a
    /// second refund of the same deposit reverts.
    pub(super) async fn refund_evm(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: RefundOptions,
        depositor: bool,
    ) -> Result<EvmReceipt> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.evm_execution.lock().await;
            let stored = self
                .get_stored_swap(id)?
                .ok_or_else(|| invalid("swap record missing"))?;
            let metadata = stored.metadata;
            let pending = pending_refund(&metadata);
            // The coordinator's refundTo pays deposits[key] whoever calls it, so
            // any funded wallet may submit it; the HTLC's refund pays msg.sender,
            // so a lock the owner is the sender of is refunded by the owner only.
            let context = EvmContext::from_response(&stored.response)?;
            let any_submitter = context.lock_sender(options.coordinator) != context.owner;
            let htlc = self
                .read_funded_evm_htlc(id, wallet, &options, pending.is_none(), any_submitter)
                .await?;
            if let Some(hash) = pending {
                return self.verified_refund(id, wallet, &htlc, parse(hash)?).await;
            }
            let resend = metadata.evm_refund_started == Some(true);
            if resend && !depositor {
                return Err(invalid(
                    "refund submission unresolved; attach its transaction, or release evm_refund_started once nothing was sent",
                ));
            }
            if !htlc.is_active(wallet).await? {
                return Err(invalid(if resend {
                    "HTLC is no longer active: the refund whose hash was not recorded, or a claim, settled it; check the depositor's balance of the lock token"
                } else {
                    "HTLC is no longer active"
                }));
            }
            if wallet.block_timestamp().await? < htlc.timelock {
                return Err(invalid("refund timelock has not expired on chain"));
            }
            // The coordinator refunds only the locks it is the sender of; from
            // HTLCErc20 v7 / HTLCNative v2 the owner is, and refunds on the HTLC.
            let via_coordinator = options
                .coordinator
                .filter(|coordinator| *coordinator == htlc.sender);
            if via_coordinator.is_none() && wallet.address() != htlc.sender {
                return Err(invalid(
                    "the lock's sender is not this wallet; the HTLC refunds only to msg.sender",
                ));
            }
            let (to, data) = match (via_coordinator, htlc.token.is_zero()) {
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
            if depositor {
                super::depositor::ensure_gas(wallet, &tx).await?;
            }
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
            self.verified_refund(id, wallet, &htlc, hash).await
        })
        .await
        .map_err(|_| {
            Error::Timeout("EVM refund timed out; inspect its stored transaction".into())
        })?
    }

    /// Refund the swap's funded EVM HTLC before its timelock, with the server's
    /// consent. On HTLCErc20 v7 / HTLCNative v2 the server signs `Refund` over
    /// the lock's terms (`POST /api/swap/{id}/collab-refund-evm/signature`) and
    /// the depositor, `wallet`, sends `refundBySig` on the HTLC: no timelock
    /// wait, the depositor pays gas. Older deployments refund early through the
    /// coordinator with a depositor-signed `CollabRefund`, which this SDK does
    /// not build; they refund after the timelock with
    /// [`Self::refund_evm_with_wallet`].
    ///
    /// The server signs only once it has decided not to fund the swap. Like the
    /// timelock refund, a submission whose hash is recorded is verified on
    /// retry, never sent twice.
    pub async fn collab_refund_evm_with_wallet(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: RefundOptions,
    ) -> Result<EvmReceipt> {
        self.collab_refund_evm(id, wallet, options, false).await
    }

    /// With `depositor`, the swap's own depositor pays the gas: the refund is
    /// refused before anything is reserved when it cannot, and a refund
    /// reserved without a recorded hash is sent again while the HTLC is active,
    /// since a second `refundBySig` of a settled lock reverts.
    pub(super) async fn collab_refund_evm(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: RefundOptions,
        depositor: bool,
    ) -> Result<EvmReceipt> {
        tokio::time::timeout(options.timeout, async {
            // A recorded submission is verified from the chain alone, so a
            // retry does not depend on the API being up.
            let _execution = self.evm_execution.lock().await;
            let metadata = self
                .get_stored_swap(id)?
                .ok_or_else(|| invalid("swap record missing"))?
                .metadata;
            let pending = pending_refund(&metadata);
            let htlc = self
                .read_funded_evm_htlc(id, wallet, &options, false, false)
                .await?;
            if let Some(hash) = pending {
                return self.verified_refund(id, wallet, &htlc, parse(hash)?).await;
            }
            if metadata.evm_refund_started == Some(true) && !depositor {
                return Err(invalid(
                    "refund submission unresolved; attach its transaction, or release evm_refund_started once nothing was sent",
                ));
            }
            let params = self.collab_refund_evm_params(id).await?;
            let kind = format!("{:?}", params.htlc_kind).to_lowercase();
            match params.refund_path.as_str() {
                "direct" => {}
                "coordinator" => {
                    return Err(invalid(format!(
                        "swap locks on {kind} v{}, whose early refund is a coordinator CollabRefund this SDK does not build; refund after the timelock",
                        params.htlc_version
                    )));
                }
                _ => {
                    return Err(invalid(format!(
                        "swap locks on {kind} v{}, which has no early refund; refund after the timelock",
                        params.htlc_version
                    )));
                }
            }
            if htlc.sender != wallet.address() {
                return Err(invalid(
                    "the lock's sender is not this wallet; only the depositor can send refundBySig",
                ));
            }
            if !htlc.is_active(wallet).await? {
                return Err(invalid("HTLC is no longer active"));
            }
            let signature = check_status(
                self.api_request(
                    reqwest::Method::POST,
                    self.api_swap_url(id, "collab-refund-evm/signature")?,
                )
                .json(&serde_json::json!({ "depositor_address": wallet.address() }))
                .send()
                .await?,
            )
            .await?
            .json::<CollabRefundEvmSignature>()
            .await?;
            // The server signs the lock as it recorded it; anything else is not
            // this lock and would revert.
            let signed = EvmHtlc {
                chain_id: signature.chain_id,
                address: parse(&signature.htlc_address)?,
                hash_lock: parse(&signature.preimage_hash)?,
                amount: U256::from_str_radix(&signature.amount, 10)
                    .map_err(|e| Error::Decode(e.to_string()))?,
                token: if htlc.token.is_zero() {
                    Address::ZERO
                } else {
                    parse(&signature.token)?
                },
                sender: parse(&signature.sender)?,
                claimer: parse(&signature.claim_address)?,
                timelock: signature.timelock,
            };
            if signed != htlc {
                return Err(invalid(
                    "the server signed a refund of other terms than this wallet's lock",
                ));
            }
            let (v, r, s) = (
                signature.v,
                parse::<B256>(&signature.r)?,
                parse::<B256>(&signature.s)?,
            );
            let data = if htlc.token.is_zero() {
                NativeHtlc::refundBySigCall {
                    preimageHash: htlc.hash_lock,
                    amount: htlc.amount,
                    timelock: U256::from(htlc.timelock),
                    v,
                    r,
                    s,
                }
                .abi_encode()
            } else {
                Htlc::refundBySigCall {
                    preimageHash: htlc.hash_lock,
                    amount: htlc.amount,
                    token: htlc.token,
                    timelock: U256::from(htlc.timelock),
                    v,
                    r,
                    s,
                }
                .abi_encode()
            };
            let tx = transaction(wallet, htlc.address, data);
            wallet.call(&tx).await?;
            if depositor {
                super::depositor::ensure_gas(wallet, &tx).await?;
            }
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
            self.verified_refund(id, wallet, &htlc, hash).await
        })
        .await
        .map_err(|_| {
            Error::Timeout("collaborative EVM refund timed out; inspect its stored transaction".into())
        })?
    }

    /// A refund that mined and reverted refunded nothing, so it is released for
    /// the next call to send anew.
    async fn verified_refund(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        htlc: &EvmHtlc,
        hash: B256,
    ) -> Result<EvmReceipt> {
        let receipt = wallet.wait_for_receipt(hash).await?;
        if receipt.transaction_hash != hash {
            return Err(invalid("refund receipt hash mismatch"));
        }
        if !receipt.success {
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_refund_started: Some(false),
                    evm_refund_reverted: Some(hash.to_string()),
                    ..Default::default()
                },
            )?;
            return Err(invalid(format!(
                "refund transaction {hash} reverted and refunded nothing; the next call sends a new one while the HTLC is active"
            )));
        }
        htlc.verify_refund_receipt(&receipt)?;
        Ok(receipt)
    }
}
