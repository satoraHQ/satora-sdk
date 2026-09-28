//! Quote discovery fixtures shared by the composition and routing tests.
use lendaswap_sdk::types::*;

pub const TBTC: &str = "0x6c84a8f1c29108f47a79964b5fe888d4f4d0de40";
pub const WBTC: &str = "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6";
pub const USDC: &str = "0x1111111111111111111111111111111111111111";

pub fn data() -> (SwapPairsResponse, NetworkFeesResponse, ChainConfigResponse) {
    let directions = [
        (Chain::bitcoin(), Chain::arbitrum()),
        (Chain::arbitrum(), Chain::bitcoin()),
        (Chain::bitcoin(), Chain::polygon()),
        (Chain::polygon(), Chain::bitcoin()),
    ];
    (
        SwapPairsResponse {
            pairs: directions
                .iter()
                .map(|(source, target)| SwapPairInfo {
                    source: source.clone(),
                    target: target.clone(),
                    min_sats: 1000,
                    max_sats: 100_000_000,
                    fee_percentage: 0.0,
                    network_fee_sats: 0,
                })
                .collect(),
        },
        NetworkFeesResponse {
            pairs: directions
                .iter()
                .map(|(source, target)| NetworkFeePairEntry {
                    source: source.clone(),
                    target: target.clone(),
                    fees: NetworkFee {
                        source_sats: 0,
                        target_sats: 0,
                    },
                })
                .collect(),
        },
        ChainConfigResponse {
            chains: vec![
                ChainConfigEntry {
                    chain: Chain::arbitrum(),
                    btc_pegged_token: TokenRef {
                        address: TBTC.into(),
                        decimals: 18,
                        symbol: "tBTC".into(),
                    },
                    btc_peg_rate: "1".into(),
                },
                ChainConfigEntry {
                    chain: Chain::polygon(),
                    btc_pegged_token: TokenRef {
                        address: WBTC.into(),
                        decimals: 8,
                        symbol: "WBTC".into(),
                    },
                    btc_peg_rate: "0.99790000".into(),
                },
            ],
        },
    )
}

pub fn request(btc_source: bool, chain: Chain, token: &str, amount: QuoteAmount) -> QuoteRequest {
    let token = TokenId::Other(token.into());
    if btc_source {
        QuoteRequest::new(Chain::bitcoin(), TokenId::Btc, chain, token, amount)
    } else {
        QuoteRequest::new(chain, token, Chain::bitcoin(), TokenId::Btc, amount)
    }
}
