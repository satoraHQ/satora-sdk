use crate::Error;
use crate::Result;
use crate::types::GetSwapResponse;
use serde::Deserialize;
use serde::Serialize;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Derivation required by an EVM swap recovered from either SDK.
/// An external address requires an external signer; it must never fall back to a local key.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvmKeySource {
    RustPerSwap,
    Bip44Wallet,
    LegacySwapKey,
    External { address: String },
}

/// Locally known execution details that the API cannot reconstruct.
/// `None` in an update leaves the existing value unchanged.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SwapMetadata {
    /// Feature-independent tracker journal; may contain EVM references even when
    /// the current process was built without EVM execution support.
    pub tracking: Option<serde_json::Value>,
    /// Non-secret bridge intents, persisted before submission.
    pub bridge_operations: Option<std::collections::BTreeMap<String, BridgeOperation>>,
    pub arkade_delegate: Option<crate::arkade::ArkadeDelegateState>,
    pub arkade_pending: Option<crate::arkade::ArkadePending>,
    pub btc_spend_txid: Option<String>,
    pub btc_spend_fee: Option<u64>,
    pub target_address: Option<String>,
    pub bridge_recipient: Option<String>,
    pub bridge_recipient_wallet: Option<String>,
    pub bridge_recipient_setup: Option<bool>,
    pub evm_fund_started: Option<bool>,
    pub evm_refund_started: Option<bool>,
    pub evm_fund_txid: Option<String>,
    pub evm_refund_txid: Option<String>,
    /// Reserved immediately before a claim can leave this process.
    pub evm_claim_started: Option<bool>,
    pub evm_claim_userop_hash: Option<String>,
    pub evm_claim_txid: Option<String>,
    pub evm_recovery_started: Option<bool>,
    pub evm_recovery_txid: Option<String>,
    /// The last recovery hash whose transfer was verified.
    pub evm_recovery_settled: Option<String>,
    /// Token, destination and amount of `evm_recovery_settled`.
    pub evm_recovery_settled_intent: Option<String>,
    pub continuation_swap_id: Option<String>,
    pub evm_fund_userop_hash: Option<String>,
    /// Server-relayed Permit2 funding, persisted before the request is sent.
    pub evm_fund_relay: Option<RelayFundingJournal>,
    pub evm_coordinator_address: Option<String>,
    pub evm_key_source: Option<EvmKeySource>,
    /// Fields written by a newer SDK, preserved across this build's rewrites.
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl SwapMetadata {
    /// The relay can still use a signed relay funding until its deadline, whatever
    /// it answered, so only a funded or expired relay journal leaves the swap free
    /// for another funding path.
    pub(crate) fn relay_signature_outstanding(&self) -> bool {
        self.evm_fund_relay.as_ref().is_some_and(|journal| {
            !matches!(
                journal.outcome,
                RelayFundingOutcome::Funded | RelayFundingOutcome::Expired
            )
        })
    }

    pub(super) fn merge(&mut self, update: Self) {
        self.extra.extend(update.extra);
        macro_rules! merge {
            ($($field:ident),* $(,)?) => {$(
                if update.$field.is_some() { self.$field = update.$field; }
                // A known name in `extra` would serialize as a duplicate JSON key.
                self.extra.remove(stringify!($field));
            )*};
        }
        merge!(
            tracking,
            bridge_operations,
            arkade_delegate,
            arkade_pending,
            btc_spend_txid,
            btc_spend_fee,
            target_address,
            bridge_recipient,
            bridge_recipient_wallet,
            bridge_recipient_setup,
            evm_fund_started,
            evm_refund_started,
            evm_fund_txid,
            evm_refund_txid,
            evm_claim_started,
            evm_claim_userop_hash,
            evm_claim_txid,
            evm_recovery_started,
            evm_recovery_txid,
            evm_recovery_settled,
            evm_recovery_settled_intent,
            continuation_swap_id,
            evm_fund_userop_hash,
            evm_fund_relay,
            evm_coordinator_address,
            evm_key_source
        );
    }
}

/// A complete swap snapshot, containing no private keys or claim preimages.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredSwap {
    pub version: u32,
    pub key_index: u32,
    pub response: GetSwapResponse,
    pub metadata: SwapMetadata,
    /// Milliseconds since the Unix epoch.
    pub stored_at: u64,
    pub updated_at: u64,
}

impl StoredSwap {
    pub fn new(key_index: u32, response: GetSwapResponse, metadata: SwapMetadata) -> Result<Self> {
        let now = now()?;
        let record = Self {
            version: 1,
            key_index,
            response,
            metadata,
            stored_at: now,
            updated_at: now,
        };
        record.validate()?;
        Ok(record)
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.key_index >= super::KEY_INDEX_LIMIT
            || self.response.id().is_empty()
        {
            return Err(Error::Storage("invalid or unsupported swap record".into()));
        }
        Ok(())
    }

    pub(super) fn merge(&mut self, newer: Self) -> Result<()> {
        newer.validate()?;
        if self.key_index != newer.key_index
            || self.response.id() != newer.response.id()
            || std::mem::discriminant(&self.response) != std::mem::discriminant(&newer.response)
        {
            return Err(Error::Storage(
                "cannot replace a stored swap's identity or key index".into(),
            ));
        }
        self.response = newer.response;
        self.metadata.merge(newer.metadata);
        self.updated_at = now()?.max(self.updated_at);
        Ok(())
    }
}

pub(super) fn now() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::Storage(e.to_string()))?
        .as_millis();
    u64::try_from(millis).map_err(|e| Error::Storage(e.to_string()))
}

/// A relayed funding attempt. Every attempt for a swap signs the same Permit2
/// nonce, and Permit2 spends a nonce once, so at most one signature can fund it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayFundingJournal {
    /// Decimal.
    pub permit2_nonce: String,
    /// The latest deadline of any signature sent with this nonce.
    pub permit2_deadline: u64,
    pub depositor: String,
    pub coordinator: String,
    pub lock_token: String,
    pub eip2612_permit: bool,
    /// Kept until the receipt proves the funding, since `evm_fund_txid` cannot be
    /// cleared once written.
    pub tx_hash: Option<String>,
    pub outcome: RelayFundingOutcome,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelayFundingOutcome {
    /// Reserved; the request may have left the process.
    Sending,
    /// Refused before the relay could broadcast the funding.
    Rejected,
    /// The relay's transaction reverted.
    Reverted,
    /// The relay returned a hash that is not yet proven on chain.
    Accepted,
    /// The request may have been processed; its answer was lost or inconclusive.
    Uncertain,
    Funded,
    /// Past the deadline with the nonce unspent: nothing can land any more.
    Expired,
}

/// Durable bridge operation. An intent without a hash means submission may have
/// succeeded before its response was lost: reconcile it, never submit blindly.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeOperation {
    pub intent: serde_json::Value,
    pub transaction_hash: Option<String>,
    pub user_operation_hash: Option<String>,
    pub complete: bool,
}
