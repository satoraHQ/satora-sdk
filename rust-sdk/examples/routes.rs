//! Route construction only. Review a selected route and explicitly call `create`
//! from your application; running this example performs no network operations.
use lendaswap_sdk::Client;
use lendaswap_sdk::Result;
use lendaswap_sdk::Swap;
use lendaswap_sdk::types::EvmAsset;
use lendaswap_sdk::types::LightningDestination;
use lendaswap_sdk::types::QuoteAmount;
use lendaswap_sdk::types::SwapCreation;
use lendaswap_sdk::types::SwapOptions;

pub struct Destinations {
    pub bitcoin: String,
    pub arkade: String,
    pub evm: String,
    pub lightning: LightningDestination,
}
/// EVM source amounts are in the source token's units. EVM target routes below
/// use a BTC source amount; Bitcoin→Arkade instead requires an exact BTC target.
pub fn routes(
    asset: EvmAsset,
    destinations: Destinations,
    evm_source_units: u128,
) -> [SwapCreation; 9] {
    [
        SwapCreation::BitcoinToArkade {
            sats_receive: 100_000,
            target_address: destinations.arkade.clone(),
        },
        SwapCreation::LightningToArkade {
            amount: QuoteAmount::Source(100_000),
            target_address: destinations.arkade.clone(),
        },
        SwapCreation::ArkadeToLightning {
            destination: destinations.lightning.clone(),
        },
        SwapCreation::BitcoinToEvm {
            target: asset.clone(),
            amount: QuoteAmount::Source(100_000),
            target_address: destinations.evm.clone(),
        },
        SwapCreation::ArkadeToEvm {
            target: asset.clone(),
            amount: QuoteAmount::Source(100_000),
            target_address: destinations.evm.clone(),
        },
        SwapCreation::LightningToEvm {
            target: asset.clone(),
            amount: QuoteAmount::Source(100_000),
            target_address: destinations.evm,
        },
        SwapCreation::EvmToBitcoin {
            source: asset.clone(),
            amount: QuoteAmount::Source(evm_source_units),
            target_address: Some(destinations.bitcoin),
        },
        SwapCreation::EvmToArkade {
            source: asset.clone(),
            amount: QuoteAmount::Source(evm_source_units),
            target_address: destinations.arkade,
        },
        SwapCreation::EvmToLightning {
            source: asset,
            destination: destinations.lightning,
        },
    ]
}
pub async fn create(
    client: &Client,
    reviewed_route: SwapCreation,
    options: SwapOptions,
) -> Result<Swap> {
    client.create_swap_request(reviewed_route, options).await
}
fn main() {
    println!(
        "This is a route construction template. Supply real destinations and explicitly select a route in your application."
    );
}
