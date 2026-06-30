/**
 * EVM utilities for Lendaswap.
 *
 * Provides helpers for encoding call data to interact with EVM HTLC contracts.
 */

export {
  buildCollabRefundEvmDigest,
  buildCollabRefundEvmTypedData,
  buildEip2612PermitDigest,
  buildPermit2FundingDigest,
  buildPermit2TypedData,
  buildRedeemCalls,
  buildRedeemDigest,
  type CollabRefundEvmDigestParams,
  type CollabRefundEvmTypedData,
  type CoordinatorCall,
  computeCoordinatorCallsHash,
  type Eip2612PermitParams,
  type ExecuteAndCreateCallData,
  type ExecuteAndCreateWithPermit2Params,
  encodeExecuteAndCreateWithPermit2,
  encodeRedeemAndExecute,
  encodeRefundAndExecute,
  encodeRefundTo,
  keccak256,
  PERMIT2_ADDRESS,
  type Permit2FundingParams,
  type Permit2SignedFundingCallData,
  type Permit2TypedData,
  type RedeemAndExecuteCallData,
  type RedeemAndExecuteParams,
  type RedeemDigestParams,
  type RefundAndExecuteParams,
  type RefundToParams,
  type UnsignedPermit2FundingData,
} from "./coordinator.js";
export {
  type ApproveCallData,
  buildEvmHtlcCallData,
  type CreateSwapCallData,
  type CreateSwapParams,
  encodeApproveCallData,
  encodeCreateSwapCallData,
  encodeHtlcErc20CreateCallData,
  encodeHtlcErc20RefundCallData,
  encodeRefundSwapCallData,
  type HtlcErc20CreateCallData,
  type HtlcErc20CreateParams,
  type HtlcErc20RefundCallData,
  type HtlcErc20RefundParams,
  type RefundSwapCallData,
  uuidToBytes32,
} from "./htlc.js";
export { deriveEvmAddress, signEvmDigest } from "./signing.js";
export {
  type EIP712TypedData,
  type EvmSigner,
  isUserRejection,
  type TxReceipt,
} from "./wallet.js";
