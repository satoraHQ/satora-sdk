/**
 * Refund module for Lendaswap swaps.
 *
 * Provides refund transaction building for different swap types:
 * - On-chain Bitcoin HTLC refunds (BTC → EVM swaps)
 * - Arkade off-chain VHTLC refunds (Arkade → EVM / Arkade → Lightning)
 */

export {
  type ArkadeRefundParams,
  type ArkadeRefundResult,
  buildArkadeRefund,
} from "./arkade.js";
export {
  type CollabRefundArkadeToEvmParams,
  type CollabRefundArkadeToEvmResult,
  collabRefundArkadeToEvmDelegate,
  collabRefundArkadeToEvmOffchain,
} from "./collab-arkade-evm.js";
export {
  type CollabRefundArkadeToLightningParams,
  type CollabRefundArkadeToLightningResult,
  collabRefundArkadeToLightningOffchain,
} from "./collab-arkade-lightning.js";
export {
  type BitcoinNetwork,
  buildOnchainClaimTransaction,
  buildOnchainRefundTransaction,
  computeHash160,
  type OnchainClaimParams,
  type OnchainClaimResult,
  type OnchainRefundParams,
  type OnchainRefundResult,
  verifyHtlcAddress,
} from "./onchain.js";
