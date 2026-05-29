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

export interface DexQuoteHop {
  from: Token;
  to: Token;
  expected_amount_in: TokenAmount;
  estimated_amount_out: TokenAmount;
  estimated_settlement_seconds: number;
  /** Per-hop router label (`"cctp"`, `"layerzero"`, `"uniswap_v3"`, `"lifi"`, …). */
  router: string;
}

export interface DexQuoteResponse {
  expected_amount_in: TokenAmount;
  estimated_amount_out: TokenAmount;
  estimated_settlement_seconds: number;
  hops: DexQuoteHop[];
  requires_multiple_user_ops: boolean;
  router: string;
  cache_ttl_seconds: number;
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

export interface WireDexQuoteHop {
  from: WireToken;
  to: WireToken;
  expected_amount_in: WireTokenAmount;
  estimated_amount_out: WireTokenAmount;
  estimated_settlement_seconds: number;
  router: string;
}

export interface WireDexQuoteResponse {
  expected_amount_in: WireTokenAmount;
  estimated_amount_out: WireTokenAmount;
  estimated_settlement_seconds: number;
  hops: WireDexQuoteHop[];
  requires_multiple_user_ops: boolean;
  router: string;
  cache_ttl_seconds: number;
}

function fromWireTokenAmount(wire: WireTokenAmount): TokenAmount {
  return {
    raw: wire.raw,
    decimals: wire.decimals,
    symbol: wire.symbol ?? undefined,
  };
}

function fromWireDexQuoteHop(wire: WireDexQuoteHop): DexQuoteHop {
  return {
    from: wire.from,
    to: wire.to,
    expected_amount_in: fromWireTokenAmount(wire.expected_amount_in),
    estimated_amount_out: fromWireTokenAmount(wire.estimated_amount_out),
    estimated_settlement_seconds: wire.estimated_settlement_seconds,
    router: wire.router,
  };
}

export function fromWireDexQuoteResponse(
  wire: WireDexQuoteResponse,
): DexQuoteResponse {
  return {
    expected_amount_in: fromWireTokenAmount(wire.expected_amount_in),
    estimated_amount_out: fromWireTokenAmount(wire.estimated_amount_out),
    estimated_settlement_seconds: wire.estimated_settlement_seconds,
    hops: wire.hops.map(fromWireDexQuoteHop),
    requires_multiple_user_ops: wire.requires_multiple_user_ops,
    router: wire.router,
    cache_ttl_seconds: wire.cache_ttl_seconds,
  };
}
