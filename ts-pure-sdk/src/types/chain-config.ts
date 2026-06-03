/**
 * `ChainConfigResponse` — per-EVM-chain metadata clients need that the
 * protocol doesn't otherwise surface (BTC-pegged pivot today; HTLC and
 * coordinator contract addresses planned). Static per session.
 *
 * Hand-written; identity-shaped with the OpenAPI codegen today.
 */
import type { Chain } from "./chain.js";

export interface TokenRef {
  /** Lowercase hex, `0x`-prefixed. */
  address: string;
  decimals: number;
  symbol: string;
}

export interface ChainConfigEntry {
  /** EVM chain id as a string (`"1"`, `"137"`, `"42161"`). */
  chain: Chain;
  /**
   * The chain's BTC-pegged token (tBTC v2 on Arbitrum / Ethereum, WBTC
   * on Polygon). Used as the DEX pivot when composing a quote for any
   * BTC↔this-chain pair.
   */
  btc_pegged_token: TokenRef;
  /**
   * How many `btc_pegged_token` units equal 1 BTC, as a decimal string.
   * `"1"` for tBTC (1:1 redeemable); the live WBTC/BTC rate (~0.998) for
   * WBTC. Used for the direct BTC↔pegged conversion (the leg with no DEX
   * hop): multiply BTC sats by this, then scale by the token's decimals.
   * Slow-moving but not static — refresh on the same cadence as a quote.
   */
  btc_peg_rate: string;
}

export interface ChainConfigResponse {
  chains: ChainConfigEntry[];
}

// -- wire shapes (identity for now) --

export interface WireTokenRef {
  address: string;
  decimals: number;
  symbol: string;
}

export interface WireChainConfigEntry {
  chain: Chain;
  btc_pegged_token: WireTokenRef;
  btc_peg_rate: string;
}

export interface WireChainConfigResponse {
  chains: WireChainConfigEntry[];
}

export function fromWireChainConfigResponse(
  wire: WireChainConfigResponse,
): ChainConfigResponse {
  return {
    chains: wire.chains.map((c) => ({
      chain: c.chain,
      btc_pegged_token: {
        address: c.btc_pegged_token.address,
        decimals: c.btc_pegged_token.decimals,
        symbol: c.btc_pegged_token.symbol,
      },
      btc_peg_rate: c.btc_peg_rate,
    })),
  };
}
