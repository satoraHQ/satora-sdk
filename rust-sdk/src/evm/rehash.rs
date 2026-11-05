use super::funding::transaction;
use super::htlc::EvmHtlc;
use super::htlc::Htlc;
use super::htlc::SwapRehashed;
use super::refund::NativeHtlc;
use super::refund::RefundOptions;
use super::*;
use crate::Client;
use crate::SwapMetadata;
use crate::client::check_status;
use crate::storage::EvmRehashRecord;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolEvent;

/// `GET /api/swap/{id}/rehash-evm/params`: the lock and what a replacement
/// invoice must satisfy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RehashEvmParams {
    pub id: String,
    pub htlc_address: String,
    pub htlc_version: i64,
    pub htlc_kind: crate::types::EvmHtlcKind,
    pub chain_id: u64,
    pub preimage_hash: String,
    pub amount: String,
    pub token: String,
    /// The depositor the lock was created under, when the server recorded it.
    pub sender: Option<String>,
    pub claim_address: String,
    pub timelock: u64,
    /// The timelock a rehash signed now would continue under.
    pub new_timelock: u64,
    /// The replacement invoice may ask for at most this many sats.
    pub max_invoice_amount_sats: u64,
    /// The replacement invoice must stay valid at least this long (seconds).
    pub min_invoice_validity_secs: u64,
}

/// `POST /api/swap/{id}/rehash-evm`: the server's `Rehash` signature with the
/// exact `rehash` arguments it is valid for; only `sender` may submit it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RehashEvmSignature {
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
    pub new_preimage_hash: String,
    pub new_timelock: u64,
    pub v: u8,
    pub r: String,
    pub s: String,
}

/// A rehash that mined: the lock continues under the new invoice's hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmRehash {
    pub tx_hash: B256,
    pub new_hash_lock: B256,
    pub new_timelock: u64,
}

/// How much later than the advertised `new_timelock` a signed one may be: a
/// rehash signed for this invoice earlier keeps its timelock, and the clock
/// moved on since the params were fetched.
const TIMELOCK_TOLERANCE_SECS: u64 = 3600;

/// The message the depositor signs (EIP-191 `personal_sign`) to authorize
/// moving swap `swap_id` to the invoice with `payment_hash`. Must match the
/// server's `rehash_evm::authorization_message`.
pub fn rehash_authorization_message(swap_id: &str, payment_hash: B256) -> String {
    format!("Satora rehash\nswap: {swap_id}\ninvoice payment hash: {payment_hash:#x}")
}

impl Client {
    /// The lock's terms and the constraints a replacement invoice must meet for
    /// a stuck EVM to Lightning swap.
    pub async fn get_rehash_evm_params(&self, id: &str) -> Result<RehashEvmParams> {
        Ok(check_status(
            self.api_request(
                reqwest::Method::GET,
                self.api_swap_url(id, "rehash-evm/params")?,
            )
            .send()
            .await?,
        )
        .await?
        .json()
        .await?)
    }

    /// Retry a stuck EVM to Lightning swap with a new invoice from the
    /// depositor, `wallet`. The server failed to pay the old invoice and gave up;
    /// instead of refunding, the lock moves to the new invoice's payment hash
    /// (HTLCErc20 v7 / HTLCNative v2 `rehash`) and the server pays that one once
    /// the move is confirmed. The wallet signs the authorization message
    /// ([`rehash_authorization_message`], via [`EvmWallet::sign_message`]) and
    /// sends `rehash`; it pays gas.
    ///
    /// The move is recorded on the swap as soon as it is broadcast, so a later
    /// [`Self::refund_evm_with_wallet`] refunds the lock that exists.
    pub async fn retry_swap_with_invoice_with_wallet(
        &self,
        id: &str,
        invoice: &str,
        wallet: &dyn EvmWallet,
        options: RefundOptions,
    ) -> Result<EvmRehash> {
        self.retry_swap_with_invoice_evm(id, invoice, wallet, options, false)
            .await
    }

    /// With `depositor`, the swap's own depositor pays the gas: the rehash is
    /// refused before anything is journaled when it cannot, so an ordinary gas
    /// shortfall leaves nothing to reconcile.
    pub(super) async fn retry_swap_with_invoice_evm(
        &self,
        id: &str,
        invoice: &str,
        wallet: &dyn EvmWallet,
        options: RefundOptions,
        depositor: bool,
    ) -> Result<EvmRehash> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.evm_execution.lock().await;
            // A rehash to this invoice already recorded is verified from its
            // receipt and returned; the server is not asked to sign again.
            let parsed = crate::client::lightning::parse_invoice(invoice)?;
            let payment_hash = B256::from_slice(
                <bitcoin::hashes::sha256::Hash as bitcoin::hashes::Hash>::as_byte_array(
                    parsed.payment_hash(),
                ),
            );
            if let Some(recorded) = self.recorded_rehash(id, wallet, payment_hash).await? {
                return Ok(recorded);
            }
            let htlc = self
                .read_funded_evm_htlc(id, wallet, &options, false, false)
                .await?;
            if htlc.sender != wallet.address() {
                return Err(invalid(
                    "the lock's sender is not this wallet; only the depositor can rehash",
                ));
            }
            if !htlc.is_active(wallet).await? {
                return Err(invalid("HTLC is no longer active"));
            }
            let params = self.get_rehash_evm_params(id).await?;
            if parse::<Address>(&params.htlc_address)? != htlc.address
                || parse::<B256>(&params.preimage_hash)? != htlc.hash_lock
                || params.timelock != htlc.timelock
                || params.chain_id != htlc.chain_id
            {
                return Err(invalid(
                    "the server describes another lock than this wallet's; refresh the swap",
                ));
            }
            // What the server advertised the invoice must satisfy; refusing here
            // saves a signature and a round trip, the server checks again.
            if let Some(msat) = parsed.amount_milli_satoshis() {
                let sats = msat / 1000;
                if sats > params.max_invoice_amount_sats {
                    return Err(invalid(format!(
                        "the invoice asks for {sats} sats, more than the swap pays out ({} sats)",
                        params.max_invoice_amount_sats
                    )));
                }
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            let expires_at = parsed.expires_at().map(|d| d.as_secs()).unwrap_or(u64::MAX);
            if expires_at < now.saturating_add(params.min_invoice_validity_secs) {
                return Err(invalid(format!(
                    "the invoice must stay valid for at least {} seconds; it expires in {}",
                    params.min_invoice_validity_secs,
                    expires_at.saturating_sub(now)
                )));
            }
            let message = rehash_authorization_message(id, payment_hash);
            let authorization = wallet.sign_message(message.as_bytes()).await?;
            let signed = check_status(
                self.api_request(reqwest::Method::POST, self.api_swap_url(id, "rehash-evm")?)
                    .json(&serde_json::json!({
                        "invoice": invoice,
                        "depositor_address": wallet.address(),
                        "depositor_signature": format!("0x{}", hex::encode(authorization.as_bytes())),
                    }))
                    .send()
                    .await?,
            )
            .await?
            .json::<RehashEvmSignature>()
            .await?;
            // The depositor authorized one payment hash; the lock may move only
            // there, for about as long as advertised, and only from this lock.
            let new_hash_lock: B256 = parse(&signed.new_preimage_hash)?;
            if new_hash_lock != payment_hash {
                return Err(invalid(
                    "the server signed a rehash to another hash than the invoice's payment hash",
                ));
            }
            if signed.new_timelock > params.new_timelock + TIMELOCK_TOLERANCE_SECS {
                return Err(invalid(format!(
                    "the server signed a rehash with timelock {}, later than the advertised {}",
                    signed.new_timelock, params.new_timelock
                )));
            }
            let signed_lock = EvmHtlc {
                chain_id: signed.chain_id,
                address: parse(&signed.htlc_address)?,
                hash_lock: parse(&signed.preimage_hash)?,
                amount: U256::from_str_radix(&signed.amount, 10)
                    .map_err(|e| Error::Decode(e.to_string()))?,
                token: if htlc.token.is_zero() {
                    Address::ZERO
                } else {
                    parse(&signed.token)?
                },
                sender: parse(&signed.sender)?,
                claimer: parse(&signed.claim_address)?,
                timelock: signed.timelock,
            };
            if signed_lock != htlc {
                return Err(invalid(
                    "the server signed a rehash of other terms than this wallet's lock",
                ));
            }
            let (v, r, s) = (
                signed.v,
                parse::<B256>(&signed.r)?,
                parse::<B256>(&signed.s)?,
            );
            let data = if htlc.token.is_zero() {
                NativeHtlc::rehashCall {
                    preimageHash: htlc.hash_lock,
                    amount: htlc.amount,
                    timelock: U256::from(htlc.timelock),
                    newPreimageHash: new_hash_lock,
                    newTimelock: U256::from(signed.new_timelock),
                    v,
                    r,
                    s,
                }
                .abi_encode()
            } else {
                Htlc::rehashCall {
                    preimageHash: htlc.hash_lock,
                    amount: htlc.amount,
                    token: htlc.token,
                    timelock: U256::from(htlc.timelock),
                    newPreimageHash: new_hash_lock,
                    newTimelock: U256::from(signed.new_timelock),
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
            // The move is journaled before it is broadcast, without its hash: a
            // broadcast whose response is lost may still mine, and a refund
            // built without the record would target the old lock. Until the
            // hash is attached or the submission released, nothing else runs.
            let mut history = self
                .get_stored_swap(id)?
                .and_then(|stored| stored.metadata.evm_rehashes)
                .unwrap_or_default();
            history.push(EvmRehashRecord {
                txid: String::new(),
                from_hash_lock: htlc.hash_lock.to_string(),
                from_timelock: htlc.timelock,
                hash_lock: new_hash_lock.to_string(),
                timelock: signed.new_timelock,
                amount: htlc.amount.to_string(),
                token: htlc.token.to_string(),
            });
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_rehashes: Some(history.clone()),
                    ..Default::default()
                },
            )?;
            let hash = wallet.send_transaction(&tx).await?;
            if let Some(last) = history.last_mut() {
                last.txid = hash.to_string();
            }
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_rehashes: Some(history.clone()),
                    ..Default::default()
                },
            )?;
            let receipt = wallet.wait_for_receipt(hash).await.map_err(|error| {
                invalid(format!(
                    "rehash {hash} is recorded but has not confirmed ({error}); call again to follow it, attach a replacement with attach_evm_rehash_transaction, or release it with release_evm_rehash_submission once it is certain it will never mine"
                ))
            })?;
            if receipt.transaction_hash != hash {
                return Err(invalid("rehash receipt hash mismatch"));
            }
            if !receipt.success {
                // Nothing moved; the record would only mislead a later refund.
                history.pop();
                self.update_swap_metadata(
                    id,
                    SwapMetadata {
                        evm_rehashes: Some(history),
                        ..Default::default()
                    },
                )?;
                return Err(invalid(format!(
                    "rehash transaction {hash} reverted; the lock is unchanged"
                )));
            }
            let moved = EvmHtlc {
                hash_lock: new_hash_lock,
                timelock: signed.new_timelock,
                ..htlc.clone()
            };
            let found = receipt
                .logs
                .iter()
                .filter(|log| log.address == htlc.address)
                .filter_map(|log| {
                    SwapRehashed::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
                        .ok()
                })
                .any(|event| {
                    event.key == htlc.key()
                        && event.newKey == moved.key()
                        && event.newPreimageHash == new_hash_lock
                });
            if !found {
                return Err(invalid(
                    "receipt has no matching SwapRehashed; the lock's terms could not be verified",
                ));
            }
            Ok(EvmRehash {
                tx_hash: hash,
                new_hash_lock,
                new_timelock: signed.new_timelock,
            })
        })
        .await
        .map_err(|_| Error::Timeout("EVM rehash timed out; inspect the swap's rehash record".into()))?
    }

    /// The recorded, mined rehash to `payment_hash`, if it is the latest move:
    /// an earlier one was superseded and the lock is elsewhere now. A recorded
    /// tx that reverted, or mined without moving the lock (a cancel), left the
    /// lock in place and is forgotten so a fresh attempt can follow; an
    /// unresolved one (no hash yet) is left to `read_funded_evm_htlc` to report.
    pub(super) async fn recorded_rehash(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        payment_hash: B256,
    ) -> Result<Option<EvmRehash>> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let mut history = stored.metadata.evm_rehashes.clone().unwrap_or_default();
        let Some(record) = history.last().cloned().filter(|rehash| {
            !rehash.txid.is_empty()
                && parse::<B256>(&rehash.hash_lock).is_ok_and(|hash| hash == payment_hash)
        }) else {
            return Ok(None);
        };
        // A record without the lock's amount and token cannot prove the move is
        // this swap's; the normal flow follows it from the funding receipt.
        if record.amount.is_empty() || record.token.is_empty() {
            return Ok(None);
        }
        let tx_hash: B256 = parse(&record.txid)?;
        let receipt = wallet.wait_for_receipt(tx_hash).await.map_err(|error| {
            invalid(format!(
                "rehash {tx_hash} is recorded but has not confirmed ({error}); attach a replacement with attach_evm_rehash_transaction, or release it with release_evm_rehash_submission once it is certain it will never mine"
            ))
        })?;
        // The move is this swap's only if the event spends this lock's key and
        // produces the recorded destination's; the same payment hash under
        // other terms is another lock.
        let context = EvmContext::from_response(&stored.response)?;
        let before = EvmHtlc {
            chain_id: context.chain_id,
            address: context.htlc,
            hash_lock: parse(&record.from_hash_lock)?,
            amount: U256::from_str_radix(&record.amount, 10)
                .map_err(|e| Error::Decode(e.to_string()))?,
            token: parse(&record.token)?,
            sender: context.owner,
            claimer: context.server,
            timelock: record.from_timelock,
        };
        let after = EvmHtlc {
            hash_lock: payment_hash,
            timelock: record.timelock,
            ..before.clone()
        };
        let events = receipt
            .logs
            .iter()
            .filter(|log| log.address == context.htlc)
            .filter_map(|log| {
                SwapRehashed::decode_raw_log_validate(log.topics.iter().copied(), &log.data).ok()
            })
            .collect::<Vec<_>>();
        let moved = receipt.success
            && events.iter().any(|event| {
                event.key == before.key()
                    && event.newKey == after.key()
                    && event.newPreimageHash == payment_hash
            });
        if receipt.success && !moved && !events.is_empty() {
            return Err(invalid(format!(
                "recorded rehash {tx_hash} moved a lock under other terms than this swap's; attach the right transaction"
            )));
        }
        if !moved {
            history.pop();
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_rehashes: Some(history),
                    ..Default::default()
                },
            )?;
            return Ok(None);
        }
        Ok(Some(EvmRehash {
            tx_hash,
            new_hash_lock: payment_hash,
            new_timelock: record.timelock,
        }))
    }

    /// Attach the hash of the swap's latest rehash: one whose broadcast response
    /// was lost, or one the wallet replaced (sped up or cancelled) under another
    /// hash. The next refund or retry verifies its receipt: a tx that reverted
    /// or did not move the lock is then skipped, one that did is followed.
    pub async fn attach_evm_rehash_transaction(&self, id: &str, hash: B256) -> Result<()> {
        let _guard = self.evm_execution.lock().await;
        if hash.is_zero() {
            return Err(invalid("no rehash submission to reconcile"));
        }
        self.resolve_rehash_submission(id, Some(hash))
    }

    /// Forget the swap's latest rehash submission, once it is certain it never
    /// mined: its broadcast response was lost and nothing went out, or its tx
    /// was dropped. A rehash that did mine and is released leaves a later
    /// refund targeting the old lock; attach its hash instead.
    pub async fn release_evm_rehash_submission(&self, id: &str) -> Result<()> {
        let _guard = self.evm_execution.lock().await;
        self.resolve_rehash_submission(id, None)
    }

    /// The record to reconcile: the one without a hash, else the latest, since
    /// only the latest move can still be pending.
    fn resolve_rehash_submission(&self, id: &str, hash: Option<B256>) -> Result<()> {
        let mut history = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?
            .metadata
            .evm_rehashes
            .unwrap_or_default();
        let unresolved = history
            .iter()
            .position(|rehash| rehash.txid.is_empty())
            .or_else(|| history.len().checked_sub(1));
        let Some(unresolved) = unresolved else {
            return Err(invalid("no rehash submission to reconcile"));
        };
        match hash {
            Some(hash) => history[unresolved].txid = hash.to_string(),
            None => {
                history.remove(unresolved);
            }
        }
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_rehashes: Some(history),
                ..Default::default()
            },
        )
    }
}
