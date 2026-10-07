use super::Client;
use super::check_status;
use super::lightning::check_new_lightning_swap;
use crate::Error;
use crate::Result;
use crate::logging::sdk_log;
use crate::types::BulkStatusResponse;
use crate::types::ChainConfigResponse;
use crate::types::GetSwapResponse;
use crate::types::MtpResponse;
use crate::types::NetworkFeesResponse;
use crate::types::RecoveryResponse;
use crate::types::ReferralFeeResponse;
use crate::types::StatusResponse;
use crate::types::SupportAgentInfo;
use crate::types::SupportAgentsResponse;
use crate::types::SwapPairsResponse;
use crate::types::TokenInfos;
use bitcoin::hashes::Hash;
use bitcoin::hashes::hash160;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::json;

// The daemon's RECOVERY_MAX_START_INDEX plus RECOVERY_MAX_SCAN (swap/src/api/recovery.rs).
// No honest page moves the cursor past this, so a larger next_index cannot
// push the persisted key counter towards exhaustion.
const RECOVERY_MAX_NEXT_INDEX: u32 = 10_000_000 + 1_000;

impl Client {
    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .api_request(Method::GET, self.url(path)?)
            .send()
            .await?;
        Ok(check_status(response).await?.json().await?)
    }

    /// Dependency health, including the valid unhealthy response sent with HTTP 503.
    pub async fn get_status(&self) -> Result<StatusResponse> {
        let response = self
            .api_request(Method::GET, self.url("status")?)
            .send()
            .await?;
        let response = if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            response
        } else {
            check_status(response).await?
        };
        Ok(response.json().await?)
    }

    /// Supported Bitcoin rails and EVM tokens.
    pub async fn get_tokens(&self) -> Result<TokenInfos> {
        self.get_json("tokens").await
    }

    /// Supported pairs, limits, and fee rates.
    pub async fn get_swap_pairs(&self) -> Result<SwapPairsResponse> {
        self.get_json("swap-pairs").await
    }

    /// EVM pivot tokens and peg rates used by quote composition.
    pub async fn get_chain_config(&self) -> Result<ChainConfigResponse> {
        self.get_json("chain-config").await
    }

    /// Current per-pair network fees in satoshis.
    pub async fn get_network_fees(&self) -> Result<NetworkFeesResponse> {
        self.get_json("network-fees").await
    }

    /// Bitcoin median time past and tip height.
    pub async fn get_mtp(&self) -> Result<MtpResponse> {
        self.get_json("mtp").await
    }

    /// Configured support agents.
    pub async fn get_support_agents(&self) -> Result<Vec<SupportAgentInfo>> {
        Ok(self
            .get_json::<SupportAgentsResponse>("support-agents")
            .await?
            .agents)
    }

    /// Resolve the referral fee, defaulting to this client's referral code.
    pub async fn get_referral_fee(
        &self,
        referral_code: Option<&str>,
        extra_fees_bps: Option<u16>,
    ) -> Result<ReferralFeeResponse> {
        let mut url = self.url("referral-fee")?;
        {
            let mut query = url.query_pairs_mut();
            if let Some(code) = referral_code.or(self.referral_code.as_deref()) {
                query.append_pair("ref", code);
            }
            if let Some(fees) = extra_fees_bps {
                query.append_pair("extra_fees", &fees.to_string());
            }
        }
        let response = self.api_request(Method::GET, url).send().await?;
        Ok(check_status(response).await?.json().await?)
    }

    /// Query up to 100 swap IDs; unknown IDs are returned in `not_found`.
    pub async fn get_bulk_status(&self, ids: &[String]) -> Result<BulkStatusResponse> {
        if ids.len() > 100 {
            return Err(Error::InvalidSwap(
                "bulk status accepts at most 100 IDs".into(),
            ));
        }
        let response = self
            .api_request(Method::POST, self.url("swap/bulk-status")?)
            .json(&json!({ "ids": ids }))
            .send()
            .await?;
        Ok(check_status(response).await?.json().await?)
    }

    /// Recover one page from the signing wallet's public recovery key.
    ///
    /// Persists swap indices and advances the allocation counter monotonically.
    /// Secrets are re-derived locally; only the recovery xpub is sent to the server.
    /// A custom [`crate::SwapStorage`] must implement `ensure_next_key_index`.
    ///
    /// Nothing from the page is stored if any swap on it fails the checks creation
    /// applies to a Lightning swap, changes the direction of a stored swap, or replaces a
    /// stored Lightning swap with one whose invoice no longer pays its hash lock; that
    /// fails with [`Error::InvalidSwap`].
    pub async fn recover_swaps(&self, start_index: Option<u32>) -> Result<RecoveryResponse> {
        let start = start_index.unwrap_or(0);
        if start >= 1 << 31 {
            return Err(Error::InvalidSwap(
                "recovery start index must be below 2^31".into(),
            ));
        }
        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| Error::InvalidSigner("recovery requires a signer".into()))?;
        let xpub = signer.derive_user_id_xpub()?.to_string();
        // Logged at info as in TypeScript: the xpub identifies the wallet but
        // cannot spend from it.
        sdk_log!(
            self.log,
            Info,
            "client.recover.start",
            "Recovering swaps for wallet",
            "xpub" => xpub,
            "startIndex" => start_index
        );
        let response = self
            .api_request(Method::POST, self.url("swap/recover")?)
            .json(&json!({ "xpub": xpub, "start_index": start }))
            .send()
            .await?;
        let recovery: RecoveryResponse = check_status(response).await?.json().await?;
        sdk_log!(
            self.log,
            Debug,
            "client.recover.response",
            "Recovered swap data from server",
            "recoveredSwapCount" => recovery.swaps.len(),
            "highestIndex" => recovery.highest_index,
            "nextIndex" => recovery.next_index,
            "scannedUntil" => recovery.scanned_until,
            "complete" => recovery.complete
        );
        if recovery.scanned_until < start
            || recovery.scanned_until >= 1 << 31
            || recovery.next_index < start
            || recovery.next_index > recovery.scanned_until + 1
            || recovery.next_index > RECOVERY_MAX_NEXT_INDEX
        {
            return Err(Error::Decode("invalid recovery cursor".into()));
        }
        // Validate the whole page before persisting anything. Invoice-owned hashes
        // cannot be compared to our preimage, only to their invoice. Check an Arkade
        // refund key too; EVM invoice swaps may belong to an externally managed depositor.
        for recovered in &recovery.swaps {
            if recovered.index < start
                || recovered.index > recovery.scanned_until
                || recovered.index >= recovery.next_index
            {
                return Err(Error::Decode(
                    "recovered index is outside the scanned range".into(),
                ));
            }
            let params = signer.derive_swap_params(recovered.index)?;
            check_new_lightning_swap(&recovered.swap, &params.hash_lock)?;
            self.check_swap_snapshot(recovered.swap.id(), &recovered.swap)?;
            let (actual, expected) = match &recovered.swap {
                GetSwapResponse::ArkadeToLightning(r) => {
                    (&r.sender_pk, hex::encode(params.public_key))
                }
                // EVM depositors can be external wallets (including TypeScript clients),
                // so they need not match this SDK's per-swap EVM derivation.
                GetSwapResponse::EvmToLightning(_) => continue,
                // The hash lock is the invoice's and the response names no key of the
                // client's, so nothing in it can show which wallet created the swap.
                GetSwapResponse::BitcoinToLightning(_) => continue,
                GetSwapResponse::BitcoinToEvm(r) => {
                    (&r.evm_hash_lock, hex::encode(params.hash_lock))
                }
                GetSwapResponse::EvmToBitcoin(r) => {
                    (&r.evm_hash_lock, hex::encode(params.hash_lock))
                }
                GetSwapResponse::BtcToArkade(r) => (
                    &r.hash_lock,
                    hash160::Hash::hash(&params.preimage).to_string(),
                ),
                GetSwapResponse::ArkadeToEvm(r) => (&r.hash_lock, hex::encode(params.hash_lock)),
                GetSwapResponse::LightningToEvm(r) => (&r.hash_lock, hex::encode(params.hash_lock)),
                GetSwapResponse::EvmToArkade(r) => (&r.hash_lock, hex::encode(params.hash_lock)),
                GetSwapResponse::LightningToArkade(r) => {
                    (&r.hash_lock, hex::encode(params.hash_lock))
                }
            };
            if !actual
                .trim_start_matches("0x")
                .eq_ignore_ascii_case(expected.trim_start_matches("0x"))
            {
                return Err(Error::InvalidSwap(format!(
                    "recovered swap {} does not match this wallet and index",
                    recovered.swap.id()
                )));
            }
        }
        self.storage.ensure_next_key_index(recovery.next_index)?;
        for recovered in &recovery.swaps {
            self.persist_server_swap(recovered.index, recovered.swap.clone())?;
        }
        Ok(recovery)
    }

    /// Follow recovery pages until the server's gap limit is reached.
    /// Successfully processed pages remain persisted if a later request fails.
    pub async fn recover_all_swaps(&self, start_index: Option<u32>) -> Result<RecoveryResponse> {
        let mut result = self.recover_swaps(start_index).await?;
        while !result.complete {
            let page = self.recover_swaps(Some(result.scanned_until + 1)).await?;
            result.swaps.extend(page.swaps);
            result.highest_index = result.highest_index.max(page.highest_index);
            result.next_index = result.next_index.max(page.next_index);
            result.scanned_until = page.scanned_until;
            result.complete = page.complete;
        }
        Ok(result)
    }
}
