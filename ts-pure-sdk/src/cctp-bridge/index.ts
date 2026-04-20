/**
 * CCTP bridge (source-side) — wraps Circle's @circle-fin/bridge-kit.
 *
 * Used for bridging USDC from any CCTPv2-supported chain into Arbitrum,
 * so it can feed into an Arbitrum USDC → BTC swap. Exposed as a subpath
 * export so bridge-kit + its viem adapter stay optional peer deps; consumers
 * that don't import from `@lendasat/lendaswap-sdk-pure/cctp-bridge` don't
 * pay the bundle cost.
 *
 * Attestation tracking for the *target* side of an existing swap (Circle's
 * forwarding service) still lives in `../cctp` — that flow isn't what
 * bridge-kit drives.
 */

export type {
  AdapterContext,
  BridgeChainIdentifier,
  BridgeConfig,
  BridgeParams,
  EstimateResult,
} from "@circle-fin/bridge-kit";

export {
  BridgeChain,
  BridgeKit,
  type BridgeResult,
  TransferSpeed,
} from "@circle-fin/bridge-kit";
export {
  type BridgeUsdcParams,
  bridgeUsdc,
  bridgeUsdcToArbitrum,
  estimateUsdcBridgeFees,
} from "./bridge.js";
