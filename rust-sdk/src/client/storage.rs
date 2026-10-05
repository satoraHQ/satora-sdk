use super::Client;
use crate::Error;
use crate::Result;
use crate::storage::EvmKeySource;
use crate::storage::StoredSwap;
use crate::storage::SwapMetadata;
use crate::storage::SwapStorage;
use crate::types::GetSwapResponse;

impl Client {
    /// Read a complete local snapshot without making a network request. An
    /// index-only store keeps no records, so the process-local copy is consulted.
    pub fn get_stored_swap(&self, id: &str) -> Result<Option<StoredSwap>> {
        match self.storage.get_swap(id)? {
            Some(record) => Ok(Some(record)),
            None => self.process_records.get_swap(id),
        }
    }

    pub fn list_all_swaps(&self) -> Result<Vec<StoredSwap>> {
        let mut records = self.storage.list_swaps()?;
        let known: std::collections::HashSet<_> =
            records.iter().map(|r| r.response.id().to_owned()).collect();
        records.extend(
            self.process_records
                .list_swaps()?
                .into_iter()
                .filter(|r| !known.contains(r.response.id())),
        );
        records.sort_by(|a, b| a.response.id().cmp(b.response.id()));
        Ok(records)
    }

    pub fn update_swap_metadata(&self, id: &str, metadata: SwapMetadata) -> Result<()> {
        if self.storage.get_swap(id)?.is_some() {
            self.storage.update_metadata(id, metadata)
        } else {
            self.process_records.update_metadata(id, metadata)
        }
    }

    /// Forget the local record. This does not cancel a swap or reset key allocation.
    pub fn delete_swap(&self, id: &str) -> Result<()> {
        self.storage.delete_swap(id)?;
        self.process_records.delete_swap(id)
    }

    /// Forget this wallet's local records without resetting key allocation.
    pub fn clear_swap_storage(&self) -> Result<()> {
        self.storage.clear()?;
        self.process_records.clear()
    }

    pub(crate) fn unreadable_swaps(&self) -> Result<Vec<(String, String)>> {
        self.storage.unreadable_swaps()
    }

    /// Refresh an existing local swap, retaining its execution metadata.
    /// Also upgrades index-only entries from older SDK databases. A response
    /// [`Self::get_swap_details`] refuses leaves the store unchanged.
    pub async fn refresh_swap(&self, id: &str) -> Result<GetSwapResponse> {
        let index = self
            .storage
            .get_swap_key_index(id)?
            .ok_or_else(|| Error::Storage("swap key index not found".into()))?;
        let response = self.get_swap_details(id).await?;
        self.persist_server_swap(index, response.clone())?;
        Ok(response)
    }

    /// Persists a server snapshot from refresh or recovery.
    pub(crate) fn persist_server_swap(&self, index: u32, response: GetSwapResponse) -> Result<()> {
        let id = response.id().to_owned();
        let journal_unknown = self.process_records.get_swap(&id)?.is_none();
        let evm_source = matches!(
            response,
            GetSwapResponse::EvmToBitcoin(_)
                | GetSwapResponse::EvmToArkade(_)
                | GetSwapResponse::EvmToLightning(_)
        );
        self.persist_swap(index, response)?;
        if journal_unknown && evm_source && self.storage.get_swap(&id)?.is_none() {
            // This store keeps no journal, so a funding sent before a restart or by
            // another Client is invisible here. Treat it as unresolved, not absent.
            self.process_records.update_metadata(
                &id,
                SwapMetadata {
                    evm_fund_started: Some(true),
                    ..Default::default()
                },
            )?;
        }
        Ok(())
    }

    pub(super) fn persist_swap(&self, index: u32, response: GetSwapResponse) -> Result<()> {
        self.persist_swap_with_metadata(index, response, SwapMetadata::default())
    }

    pub(super) fn persist_swap_with_metadata(
        &self,
        index: u32,
        response: GetSwapResponse,
        mut metadata: SwapMetadata,
    ) -> Result<()> {
        let address = match &response {
            GetSwapResponse::EvmToBitcoin(r) => Some(&r.client_evm_address),
            GetSwapResponse::EvmToArkade(r) => Some(&r.client_evm_address),
            GetSwapResponse::EvmToLightning(r) => Some(&r.client_evm_address),
            GetSwapResponse::BitcoinToEvm(r) => Some(&r.client_evm_address),
            GetSwapResponse::ArkadeToEvm(r) => Some(&r.client_evm_address),
            GetSwapResponse::LightningToEvm(r) => Some(&r.client_evm_address),
            _ => None,
        };
        let evm_key_source = address
            .map(|address| -> Result<EvmKeySource> {
                let signer = self.signer.as_ref().ok_or_else(|| {
                    Error::InvalidSigner("persisting EVM ownership requires a signer".into())
                })?;
                if signer
                    .derive_evm_key(index)?
                    .address
                    .eq_ignore_ascii_case(address)
                {
                    Ok(EvmKeySource::RustPerSwap)
                } else if signer
                    .derive_wallet_evm_key()?
                    .address
                    .eq_ignore_ascii_case(address)
                {
                    Ok(EvmKeySource::Bip44Wallet)
                } else if signer
                    .derive_legacy_evm_key(index)?
                    .address
                    .eq_ignore_ascii_case(address)
                {
                    Ok(EvmKeySource::LegacySwapKey)
                } else {
                    Ok(EvmKeySource::External {
                        address: address.clone(),
                    })
                }
            })
            .transpose()?;
        metadata.evm_key_source = evm_key_source;
        let record = StoredSwap::new(index, response, metadata)?;
        self.storage.store_swap(record.clone())?;
        if self.storage.get_swap(record.response.id())?.is_none() {
            self.process_records.store_swap(record)?;
        }
        Ok(())
    }
}
