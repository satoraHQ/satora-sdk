/**
 * `NetworkFeesResponse` — per-pair gas/mining sats at current prices.
 *
 * Hand-written; identity-shaped with the OpenAPI codegen today.
 */
import type { Chain } from "./chain.js";

export interface NetworkFee {
  /**
   * Sats incurred on the source chain (BTC mining when source is
   * Bitcoin; EVM gas-to-sats for HTLC claim when source is an EVM
   * chain; `0` for Arkade / Lightning).
   */
  source_sats: number;
  /**
   * Sats incurred on the target chain (EVM gas-to-sats for HTLC create
   * when target is an EVM chain; BTC mining when target is Bitcoin;
   * `0` for Arkade / Lightning).
   */
  target_sats: number;
}

export interface NetworkFeePairEntry {
  source: Chain;
  target: Chain;
  fees: NetworkFee;
}

export interface NetworkFeesResponse {
  pairs: NetworkFeePairEntry[];
}

export interface WireNetworkFee {
  source_sats: number;
  target_sats: number;
}

export interface WireNetworkFeePairEntry {
  source: Chain;
  target: Chain;
  fees: WireNetworkFee;
}

export interface WireNetworkFeesResponse {
  pairs: WireNetworkFeePairEntry[];
}

export function fromWireNetworkFeePairEntry(
  wire: WireNetworkFeePairEntry,
): NetworkFeePairEntry {
  return {
    source: wire.source,
    target: wire.target,
    fees: {
      source_sats: wire.fees.source_sats,
      target_sats: wire.fees.target_sats,
    },
  };
}

export function fromWireNetworkFeesResponse(
  wire: WireNetworkFeesResponse,
): NetworkFeesResponse {
  return { pairs: wire.pairs.map(fromWireNetworkFeePairEntry) };
}
