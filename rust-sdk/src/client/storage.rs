use super::Client;
use crate::Error;
use crate::Result;
use crate::storage::EvmKeySource;
use crate::storage::StoredSwap;
use crate::storage::SwapMetadata;
use crate::types::GetSwapResponse;

impl Client {
    /// Read a complete local snapshot without making a network request.
    pub fn get_stored_swap(&self, id: &str) -> Result<Option<StoredSwap>> {
        self.storage.get_swap(id)
    }

    pub fn list_all_swaps(&self) -> Result<Vec<StoredSwap>> {
        self.storage.list_swaps()
    }

    pub fn update_swap_metadata(&self, id: &str, metadata: SwapMetadata) -> Result<()> {
        self.storage.update_metadata(id, metadata)
    }

    /// Forget the local record. This does not cancel a swap or reset key allocation.
    pub fn delete_swap(&self, id: &str) -> Result<()> {
        self.storage.delete_swap(id)
    }

    /// Forget this wallet's local records without resetting key allocation.
    pub fn clear_swap_storage(&self) -> Result<()> {
        self.storage.clear()
    }

    /// Refresh an existing local swap, retaining its execution metadata.
    /// Also upgrades index-only entries from older SDK databases.
    pub async fn refresh_swap(&self, id: &str) -> Result<GetSwapResponse> {
        let index = self
            .storage
            .get_swap_key_index(id)?
            .ok_or_else(|| Error::Storage("swap key index not found".into()))?;
        let response = self.get_swap_details(id).await?;
        if response.id() != id {
            return Err(Error::Decode(
                "response swap ID does not match request".into(),
            ));
        }
        self.persist_swap(index, response.clone())?;
        Ok(response)
    }

    pub(super) fn persist_swap(&self, index: u32, response: GetSwapResponse) -> Result<()> {
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
        self.storage.store_swap(StoredSwap::new(
            index,
            response,
            SwapMetadata {
                evm_key_source,
                ..Default::default()
            },
        )?)
    }
}
