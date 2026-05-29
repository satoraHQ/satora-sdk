/**
 * `QuoteResponse` — end-to-end pricing for a swap between BTC and an
 * EVM/Arkade/Lightning target.
 *
 * Hand-written rather than re-exported from the OpenAPI codegen so the
 * SDK surface stays stable across wire-format evolution. The shape is
 * currently identity-mapped with what `GET /quote` returns; future
 * SDK-only additions land here as `?:` fields without requiring a
 * server-side schema bump.
 *
 * Numeric u64 fields ship as plain `number` (the wire serializes them as
 * JSON numbers, and BTC sats easily fit under 2^53). Stringified u128
 * fields (`source_amount`, `target_amount`, …) ship as `string` to dodge
 * the JS `Number` precision cliff for 18-decimal tokens.
 */
export interface QuoteResponse {
  /** Exchange rate: how much of the EVM token you get/pay per BTC. */
  exchange_rate: string;
  /**
   * Network fee estimate (in satoshis) — covers server-paid gas for
   * HTLC create/claim + BTC mining fee when applicable.
   */
  network_fee: number;
  /**
   * Gasless network fee estimate (in satoshis) — covers the additional
   * gas the server pays to execute the DEX swap on behalf of the user
   * (redeemAndExecute via the coordinator contract).
   */
  gasless_network_fee: number;
  /** Protocol fee (in satoshis). */
  protocol_fee: number;
  /** Protocol fee rate (as decimal, e.g., 0.0025 = 0.25%). */
  protocol_fee_rate: number;
  /** Minimum BTC value of the swap in satoshis. */
  min_amount: number;
  /** Maximum BTC value of the swap in satoshis. */
  max_amount: number;
  /** Pre-calculated source amount in smallest unit of source token (pre-fee). */
  source_amount: string;
  /** Pre-calculated target amount in smallest unit of target token (pre-fee). */
  target_amount: string;
  /**
   * Net source amount: what the user actually sends including all fees.
   * Equals `source_amount` when the user provided `source_amount`;
   * otherwise `source_amount + fees_in_source_units`.
   */
  net_source_amount: string;
  /**
   * Net target amount: what the user actually receives after all fees.
   * Equals `target_amount` when the user provided `target_amount`;
   * otherwise `target_amount - fees_in_target_units`.
   */
  net_target_amount: string;
  /**
   * CCTP bridge forwarding fee in USDC smallest units (6 decimals).
   * Only present when `bridge_target_chain` was specified in the quote
   * request, or for inbound CCTP swaps.
   */
  bridge_fee?: number | null;
}

/**
 * Wire payload shape for `GET /quote` — matches the OpenAPI-generated
 * type. Internal use only.
 */
export interface WireQuoteResponse {
  exchange_rate: string;
  network_fee: number;
  gasless_network_fee: number;
  protocol_fee: number;
  protocol_fee_rate: number;
  min_amount: number;
  max_amount: number;
  source_amount: string;
  target_amount: string;
  net_source_amount: string;
  net_target_amount: string;
  bridge_fee?: number | null;
}

/**
 * Translate the wire payload into the SDK-facing type.
 *
 * Identity-shaped today. Lives as a function (not just a cast) so future
 * wire-vs-SDK divergence (renames, type changes, normalization) has an
 * obvious place to land without touching call sites.
 */
export function fromWireQuoteResponse(wire: WireQuoteResponse): QuoteResponse {
  return {
    exchange_rate: wire.exchange_rate,
    network_fee: wire.network_fee,
    gasless_network_fee: wire.gasless_network_fee,
    protocol_fee: wire.protocol_fee,
    protocol_fee_rate: wire.protocol_fee_rate,
    min_amount: wire.min_amount,
    max_amount: wire.max_amount,
    source_amount: wire.source_amount,
    target_amount: wire.target_amount,
    net_source_amount: wire.net_source_amount,
    net_target_amount: wire.net_target_amount,
    bridge_fee: wire.bridge_fee ?? undefined,
  };
}
