/**
 * CCTP-inbound flow — "any CCTPv2 chain → Arbitrum USDC → BTC" swaps.
 *
 * The SDK orchestrates:
 *   1. Source-chain USDC burn via `TokenMessenger.depositForBurn`
 *      (`destinationCaller` pinned to the user's Kernel smart account).
 *   2. IRIS attestation polling.
 *   3. Settlement-chain UserOp via the user's Kernel smart account
 *      (atomic `receiveMessage` + `USDC.approve(Permit2)` +
 *      `executeAndCreateWithPermit2`), paymaster-sponsored so the user
 *      needs no ETH on Arbitrum.
 *
 * The Kernel smart account is owned by the user's connected wallet
 * (viem `Account` / Privy / wagmi / raw key) — no SDK-derived key
 * material. Consumer-side wallet prompts cover Permit2 typed data and
 * the UserOp signature.
 *
 * Public API:
 *   - `client.cctpInbound.*` — step-by-step primitives (for custom UX)
 *   - `client.fundSwap(swapId, signer)` — one-shot wrapper built on top
 */

export {
  type ApproveAndBurnParams,
  type ApproveAndBurnResult,
  approveAndBurn,
} from "./approveAndBurn.js";
export {
  type DepositForBurnParams,
  encodeDepositForBurn,
} from "./burn.js";
export type { CctpInboundClientConfig } from "./client.js";
export { CctpInboundClient } from "./client.js";
export {
  buildDomainSeparator,
  MAX_UINT256,
  type SignEip2612Params,
  type SignedEip2612Permit,
  signEip2612Permit,
  USDC_DOMAIN_FIELDS,
} from "./eip2612.js";
export {
  type CctpFundSwapContext,
  type CctpFundSwapParams,
  type CctpFundSwapResult,
  type CctpProgressStep,
  cctpFundSwap,
} from "./fundSwap.js";
export {
  type SignedPermit2Witness,
  type SignPermit2Params,
  signPermit2Witness,
} from "./permit2.js";
export {
  extractRevertData,
  type SimulateBatchCallsArgs,
  simulateBatchCalls,
} from "./preflight.js";
export {
  type CreateSwapSmartAccountClientParams,
  createSwapSmartAccountClient,
} from "./smartAccount.js";
export {
  type SubmitUserOpContext,
  type SubmitUserOpParams,
  type SubmitUserOpResult,
  submitCctpInboundUserOp,
} from "./submit.js";
export type { AaConfig } from "./types.js";
export {
  addressToBytes32Hex,
  type BatchCall,
  type BuildCctpInboundBatchParams,
  type BuiltBatch,
  buildCctpInboundBatch,
  type SignTypedDataFn,
  type UseropCalldataResponse,
} from "./userOp.js";
