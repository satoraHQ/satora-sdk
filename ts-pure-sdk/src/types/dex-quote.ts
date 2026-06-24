/**
 * `DexQuoteResponse` — DEX-leg pricing for a same-chain EVM swap.
 *
 * Hand-written; identity-shaped with the OpenAPI codegen today. The
 * shape covers the response of `POST /dex-quote` plus a `Token` /
 * `TokenAmount` / `DexQuoteAmount` value type the request and response
 * share.
 */

/**
 * Asset identifier. EVM today; Solana variant reserved for future use.
 *
 * Tagged on `kind` so consumers can switch and the type widens
 * additively as new chain kinds land.
 */
export type Token =
  | { kind: "evm"; chain_id: number; address: string }
  | { kind: "solana"; address: string };

/**
 * Pinned-side amount on the wire — direction + smallest-unit value.
 */
export type DexQuoteAmount =
  | { kind: "exact_in"; value: string }
  | { kind: "exact_out"; value: string };

/**
 * Smallest-unit amount paired with the associated token's symbol and
 * decimal places.
 */
export interface TokenAmount {
  raw: string;
  decimals: number;
  symbol?: string;
}

/**
 * The bridge protocol a {@link BridgeRate} is for — the `router` discriminant
 * of the union. Switch on it exhaustively.
 */
export type BridgeRouter = "cctp" | "layerzero";

/**
 * Fee model for the cross-chain bridge leg of a quote, tagged by protocol
 * (`router`). Present for any bridged quote; each protocol is a variant with
 * exactly its own fields, so `switch` on `router` rather than assuming a shape.
 *
 * The bridge settles inside the single Arbitrum `Call[]` the SDK submits (no
 * extra user op) — this just says what (if anything) to subtract from the
 * DEX-leg output to get the delivered amount.
 *
 * - **`cctp`**: Circle's Forwarding Service deducts a percentage (`minimumFee`)
 *   + flat (`forwardFee`) from the bridged USDC. Apply, on a 6-dec USDC
 *   `amount`:
 *   ```text
 *   protocol = floor(floor(amount * minimum_fee_scaled / 1_000_000) * 120 / 100)
 *   fee      = protocol + (recipientSetup ? flat_with_setup : flat)
 *   ```
 *   Mirrors the server's `cctp::compute_max_fee` bit-for-bit.
 * - **`layerzero`**: no token-denominated deduction — the messaging fee is ETH
 *   the user pays via their own (Alchemy) publish, never taken from the USDT0.
 *   Nothing to subtract today.
 */
export type BridgeRate =
  | {
      router: "cctp";
      /**
       * The token every amount in this variant is denominated in
       * (`flat`, `flat_with_setup`, and the `amount` the percentage applies
       * to) — the remote bridged USDC from the request. Check it matches the
       * token you're deducting from before applying; a future asset-denominated
       * fee would need a conversion.
       */
      fee_token: Token;
      /** Circle's `minimumFee` pre-scaled to `round(minimumFee_bps * 100)`. Stringified u128. */
      minimum_fee_scaled: string;
      /** Flat USDC fee deducted from the bridged amount, recipient already provisioned. Outbound: `forwardFee.high`; inbound: `"0"`. Stringified u64. */
      flat: string;
      /** Flat-fee variant including a fresh non-EVM recipient's ATA rent (Solana). Equals `flat` otherwise; same units. Stringified u64. */
      flat_with_setup: string;
    }
  | { router: "layerzero" };

export interface DexQuoteResponse {
  /** End-to-end input amount the user sends (incl. any bridge gross-up for exact-out). */
  expected_amount_in: TokenAmount;
  /** End-to-end output amount the user receives (net of any bridge fee for exact-in). */
  estimated_amount_out: TokenAmount;
  /** Rough end-to-end settlement ETA in seconds; `0` for a same-chain swap. */
  estimated_settlement_seconds: number;
  /** Router that priced the DEX leg (`"uniswap_v3"`, `"lifi"`, …). Informational. */
  router: string;
  cache_ttl_seconds: number;
  /**
   * Cross-chain bridge fee model, tagged by protocol. Present for any bridged
   * quote — `{ router: "cctp", … }` (a USDC deduction) or
   * `{ router: "layerzero" }` (no deduction); `undefined` only for plain
   * same-chain quotes.
   */
  bridge_rate?: BridgeRate;
}

// -- wire shapes (identical to the SDK types for now; explicit so we
//    can let the wire diverge later without touching SDK call sites) --

export type WireToken = Token;
export type WireDexQuoteAmount = DexQuoteAmount;

export interface WireTokenAmount {
  raw: string;
  decimals: number;
  symbol?: string | null;
}

export type WireBridgeRate =
  | {
      router: "cctp";
      fee_token: WireToken;
      minimum_fee_scaled: string;
      flat: string;
      flat_with_setup: string;
    }
  | { router: "layerzero" };

export interface WireDexQuoteResponse {
  expected_amount_in: WireTokenAmount;
  estimated_amount_out: WireTokenAmount;
  estimated_settlement_seconds: number;
  router: string;
  cache_ttl_seconds: number;
  bridge_rate?: WireBridgeRate | null;
}

function fromWireTokenAmount(wire: WireTokenAmount): TokenAmount {
  return {
    raw: wire.raw,
    decimals: wire.decimals,
    symbol: wire.symbol ?? undefined,
  };
}

export function fromWireDexQuoteResponse(
  wire: WireDexQuoteResponse,
): DexQuoteResponse {
  return {
    expected_amount_in: fromWireTokenAmount(wire.expected_amount_in),
    estimated_amount_out: fromWireTokenAmount(wire.estimated_amount_out),
    estimated_settlement_seconds: wire.estimated_settlement_seconds,
    router: wire.router,
    cache_ttl_seconds: wire.cache_ttl_seconds,
    bridge_rate: wire.bridge_rate
      ? fromWireBridgeRate(wire.bridge_rate)
      : undefined,
  };
}

function fromWireBridgeRate(wire: WireBridgeRate): BridgeRate {
  if (wire.router === "cctp") {
    return {
      router: "cctp",
      fee_token: wire.fee_token,
      minimum_fee_scaled: wire.minimum_fee_scaled,
      flat: wire.flat,
      flat_with_setup: wire.flat_with_setup,
    };
  }
  return { router: "layerzero" };
}
