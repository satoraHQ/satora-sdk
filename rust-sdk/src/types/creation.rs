use super::LightningDestination;
use super::QuoteAmount;
use super::TokenId;

/// An EVM asset with an explicit chain, including tokens not named by the SDK.
#[derive(Clone, Debug)]
pub struct EvmAsset {
    pub chain_id: u64,
    pub token: TokenId,
}

/// Bridge routing declared at creation. Execution is a separate operation.
#[derive(Clone, Debug)]
pub struct BridgeRoute {
    pub chain: String,
    pub token_address: String,
    /// Outbound recipient, e.g. a Solana USDC associated token account.
    pub recipient: Option<String>,
    /// Wallet needed when creating an outbound Solana token account.
    pub recipient_wallet: Option<String>,
    pub recipient_setup: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SwapOptions {
    /// Defaults to true for Bitcoin/Arkade/Lightning→EVM, false for EVM sources.
    pub gasless: Option<bool>,
    pub extra_fees_bps: Option<u16>,
    pub invoice_description: Option<String>,
    /// Source bridge for EVM inputs, target bridge for EVM outputs.
    pub bridge: Option<BridgeRoute>,
    /// External depositor/claim signer. Omit to use the Rust per-swap key.
    pub evm_address: Option<String>,
}

/// Explicit routes avoid inferring whether BTC means Bitcoin, Arkade or Lightning.
/// Amounts use smallest token units. Bitcoin→Arkade accepts an exact target only,
/// matching the API. Invoice payouts carry their amount in the invoice itself.
#[derive(Clone, Debug)]
pub enum SwapCreation {
    BitcoinToArkade {
        sats_receive: u64,
        target_address: String,
    },
    LightningToArkade {
        amount: QuoteAmount,
        target_address: String,
    },
    ArkadeToLightning {
        destination: LightningDestination,
    },
    BitcoinToEvm {
        target: EvmAsset,
        amount: QuoteAmount,
        target_address: String,
    },
    ArkadeToEvm {
        target: EvmAsset,
        amount: QuoteAmount,
        target_address: String,
    },
    LightningToEvm {
        target: EvmAsset,
        amount: QuoteAmount,
        target_address: String,
    },
    EvmToBitcoin {
        source: EvmAsset,
        amount: QuoteAmount,
        target_address: Option<String>,
    },
    EvmToArkade {
        source: EvmAsset,
        amount: QuoteAmount,
        target_address: String,
    },
    EvmToLightning {
        source: EvmAsset,
        destination: LightningDestination,
    },
}
