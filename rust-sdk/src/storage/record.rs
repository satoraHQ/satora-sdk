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
    pub evm_fund_txid: Option<String>,
    pub evm_refund_txid: Option<String>,
    /// Reserved immediately before a claim can leave this process.
    pub evm_claim_started: Option<bool>,
    pub evm_claim_userop_hash: Option<String>,
    pub evm_claim_txid: Option<String>,
    pub evm_recovery_txid: Option<String>,
    pub continuation_swap_id: Option<String>,
    pub evm_fund_userop_hash: Option<String>,
    pub evm_coordinator_address: Option<String>,
    pub evm_key_source: Option<EvmKeySource>,
}

impl SwapMetadata {
    pub(super) fn merge(&mut self, update: Self) {
        macro_rules! merge {
            ($($field:ident),* $(,)?) => {$(
                if update.$field.is_some() { self.$field = update.$field; }
            )*};
        }
        merge!(
            bridge_operations,
            arkade_delegate,
            arkade_pending,
            btc_spend_txid,
            btc_spend_fee,
            target_address,
            bridge_recipient,
            bridge_recipient_wallet,
            bridge_recipient_setup,
            evm_fund_txid,
            evm_refund_txid,
            evm_claim_started,
            evm_claim_userop_hash,
            evm_claim_txid,
            evm_recovery_txid,
            continuation_swap_id,
            evm_fund_userop_hash,
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

/// Durable bridge operation. An intent without a hash means submission may have
/// succeeded before its response was lost: reconcile it, never submit blindly.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeOperation {
    pub intent: serde_json::Value,
    pub transaction_hash: Option<String>,
    pub user_operation_hash: Option<String>,
    pub complete: bool,
}
