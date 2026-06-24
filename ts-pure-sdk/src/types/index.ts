/**
 * Hand-written SDK types — the public type surface.
 *
 * Decoupled from `src/generated/*` (OpenAPI codegen, internal only) so
 * the SDK contract stays stable across wire-format evolution. Each
 * domain module exports the public type and a `fromWire*()` translator
 * used by `Client` methods to coerce HTTP responses into the SDK shape.
 */

export type { Chain } from "./chain.js";
export {
  type ChainConfigEntry,
  type ChainConfigResponse,
  fromWireChainConfigResponse,
  type TokenRef,
  type WireChainConfigResponse,
} from "./chain-config.js";
export {
  type BridgeRate,
  type BridgeRouter,
  type DexQuoteAmount,
  type DexQuoteHop,
  type DexQuoteResponse,
  fromWireDexQuoteResponse,
  type Token,
  type TokenAmount,
  type WireDexQuoteResponse,
} from "./dex-quote.js";
export {
  fromWireNetworkFeePairEntry,
  fromWireNetworkFeesResponse,
  type NetworkFee,
  type NetworkFeePairEntry,
  type NetworkFeesResponse,
  type WireNetworkFeesResponse,
} from "./network-fees.js";
export {
  fromWireQuoteResponse,
  type QuoteResponse,
  type WireQuoteResponse,
} from "./quote.js";
export {
  fromWireSwapPairsResponse,
  type SwapPairInfo,
  type SwapPairsResponse,
  type WireSwapPairsResponse,
} from "./swap-pairs.js";
