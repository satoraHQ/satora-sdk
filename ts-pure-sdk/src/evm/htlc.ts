/**
 * EVM HTLC encoding utilities.
 *
 * Provides helpers for encoding call data to interact with the
 * ReverseAtomicSwapHTLC contract for EVM-to-BTC swaps.
 */

import { keccak_256 } from "@noble/hashes/sha3.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import { decodeUint256, type ReceiptLog } from "./wallet.js";

/**
 * Parameters for creating an EVM HTLC swap.
 */
export interface CreateSwapParams {
  /** Unique swap identifier (UUID or bytes32 hex) */
  swapId: string;
  /** Address to receive tokens on claim (Lendaswap's address) */
  recipient: string;
  /** Input token address (e.g., USDC) */
  tokenIn: string;
  /** Output token address (e.g., WBTC) */
  tokenOut: string;
  /** Amount of input token (in token's smallest unit, e.g., 6 decimals for USDC) */
  amountIn: bigint;
  /** Hash lock (SHA256 of preimage, 32 bytes hex with 0x prefix) */
  hashLock: string;
  /** Unix timestamp after which refund is possible */
  timelock: number;
  /** Uniswap pool fee tier (500, 3000, or 10000) */
  poolFee: number;
  /** Minimum output amount (slippage protection) */
  minAmountOut: bigint;
}

/**
 * Result of encoding createSwap call data.
 */
export interface CreateSwapCallData {
  /** The HTLC contract address to call */
  to: string;
  /** The encoded call data */
  data: string;
  /** Human-readable function signature */
  functionSignature: string;
}

/**
 * Result of encoding approve call data.
 */
export interface ApproveCallData {
  /** The token contract address to call */
  to: string;
  /** The encoded call data */
  data: string;
  /** Human-readable function signature */
  functionSignature: string;
}

/**
 * Result of encoding refundSwap call data.
 */
export interface RefundSwapCallData {
  /** The HTLC contract address to call */
  to: string;
  /** The encoded call data */
  data: string;
  /** Human-readable function signature */
  functionSignature: string;
}

/**
 * Parameters for creating an HTLCErc20 swap (new contract).
 */
export interface HtlcErc20CreateParams {
  /** Hash lock (SHA256 of preimage, 32 bytes hex with 0x prefix) */
  preimageHash: string;
  /** Amount of tokens (in token's smallest unit) */
  amount: bigint;
  /** Token contract address */
  token: string;
  /** Address that can refund (usually the sender/user) */
  sender: string;
  /** Address that can claim with preimage (usually the server) */
  claimer: string;
  /** Unix timestamp after which refund is possible */
  timelock: number;
}

/**
 * Result of encoding HTLCErc20 create call data.
 */
export interface HtlcErc20CreateCallData {
  /** The HTLC contract address to call */
  to: string;
  /** The encoded call data */
  data: string;
  /** Human-readable function signature */
  functionSignature: string;
}

// Function selectors (first 4 bytes of keccak256 hash of function signature)
// createSwap(bytes32,address,address,address,uint256,bytes32,uint256,uint24,uint256)
const CREATE_SWAP_SELECTOR = "0x9a4efe51";
// approve(address,uint256)
const APPROVE_SELECTOR = "0x095ea7b3";
// refundSwap(bytes32)
const REFUND_SWAP_SELECTOR = "0xfe2510ee";
// HTLCErc20.create(bytes32,uint256,address,address,address,uint256)
const HTLC_ERC20_CREATE_SELECTOR = "0x06799dee";
// HTLCErc20.refund(bytes32,uint256,address,address,uint256)
const HTLC_ERC20_REFUND_SELECTOR = "0x36504721";
// HTLCErc20.isActive(bytes32,uint256,address,address,address,uint256)
const HTLC_ERC20_IS_ACTIVE_SELECTOR = "0x160c723c";

/**
 * Converts a UUID string to a bytes32 hex string (right-padded with zeros).
 *
 * @param uuid - The UUID string (with or without hyphens)
 * @returns The bytes32 hex string (without 0x prefix)
 */
export function uuidToBytes32(uuid: string): string {
  // Remove hyphens and 0x prefix if present
  let cleanHex = uuid.replace(/-/g, "").replace(/^0x/, "");

  // Ensure it's valid hex
  if (!/^[0-9a-fA-F]+$/.test(cleanHex)) {
    throw new Error(`Invalid UUID hex: ${uuid}`);
  }

  // Right-pad to 64 characters (32 bytes)
  if (cleanHex.length < 64) {
    cleanHex = cleanHex.padEnd(64, "0");
  } else if (cleanHex.length > 64) {
    cleanHex = cleanHex.slice(0, 64);
  }

  return cleanHex.toLowerCase();
}

/**
 * Normalizes a bytes32 value (removes 0x prefix, ensures 64 chars).
 */
export function normalizeBytes32(value: string): string {
  let clean = value.replace(/^0x/, "");
  if (clean.length < 64) {
    clean = clean.padStart(64, "0");
  }
  return clean.toLowerCase();
}

/**
 * Normalizes an address to 32 bytes (left-padded with zeros).
 */
function normalizeAddress(address: string): string {
  const clean = address.replace(/^0x/, "").toLowerCase();
  return clean.padStart(64, "0");
}

/**
 * Encodes a uint256 value as 32 bytes hex.
 */
function encodeUint256(value: bigint): string {
  return value.toString(16).padStart(64, "0");
}

/**
 * Encodes a uint24 value as 32 bytes hex (for ABI encoding).
 */
function encodeUint24(value: number): string {
  return value.toString(16).padStart(64, "0");
}

/**
 * Encodes the call data for ERC20 approve function.
 *
 * @param spender - The address to approve (HTLC contract)
 * @param amount - The amount to approve
 * @returns The encoded approve call data
 *
 * @example
 * ```ts
 * const approveData = encodeApproveCallData(htlcAddress, amountIn);
 * // Use with ethers.js:
 * await tokenContract.sendTransaction({ to: tokenAddress, data: approveData.data });
 * ```
 */
export function encodeApproveCallData(
  tokenAddress: string,
  spender: string,
  amount: bigint,
): ApproveCallData {
  const spenderEncoded = normalizeAddress(spender);
  const amountEncoded = encodeUint256(amount);

  const data = `${APPROVE_SELECTOR}${spenderEncoded}${amountEncoded}`;

  return {
    to: tokenAddress,
    data,
    functionSignature: "approve(address,uint256)",
  };
}

/**
 * Encodes the call data for createSwap function on the HTLC contract.
 *
 * @param htlcAddress - The HTLC contract address
 * @param params - The createSwap parameters
 * @returns The encoded createSwap call data
 *
 * @example
 * ```ts
 * const createData = encodeCreateSwapCallData(htlcAddress, {
 *   swapId: "12345678-1234-1234-1234-123456789abc",
 *   recipient: "0x...",
 *   tokenIn: "0x...", // USDC
 *   tokenOut: "0x...", // WBTC
 *   amountIn: 100000000n, // 100 USDC (6 decimals)
 *   hashLock: "0x...",
 *   timelock: Math.floor(Date.now() / 1000) + 3600,
 *   poolFee: 3000,
 *   minAmountOut: 0n,
 * });
 * ```
 */
export function encodeCreateSwapCallData(
  htlcAddress: string,
  params: CreateSwapParams,
): CreateSwapCallData {
  // Encode each parameter
  const swapIdEncoded = uuidToBytes32(params.swapId);
  const recipientEncoded = normalizeAddress(params.recipient);
  const tokenInEncoded = normalizeAddress(params.tokenIn);
  const tokenOutEncoded = normalizeAddress(params.tokenOut);
  const amountInEncoded = encodeUint256(params.amountIn);
  const hashLockEncoded = normalizeBytes32(params.hashLock);
  const timelockEncoded = encodeUint256(BigInt(params.timelock));
  const poolFeeEncoded = encodeUint24(params.poolFee);
  const minAmountOutEncoded = encodeUint256(params.minAmountOut);

  const data = [
    CREATE_SWAP_SELECTOR,
    swapIdEncoded,
    recipientEncoded,
    tokenInEncoded,
    tokenOutEncoded,
    amountInEncoded,
    hashLockEncoded,
    timelockEncoded,
    poolFeeEncoded,
    minAmountOutEncoded,
  ].join("");

  return {
    to: htlcAddress,
    data,
    functionSignature:
      "createSwap(bytes32,address,address,address,uint256,bytes32,uint256,uint24,uint256)",
  };
}

/**
 * Encodes the call data for refundSwap function on the HTLC contract.
 *
 * @param htlcAddress - The HTLC contract address
 * @param swapId - The swap ID (UUID or bytes32 hex)
 * @returns The encoded refundSwap call data
 *
 * @example
 * ```ts
 * const refundData = encodeRefundSwapCallData(htlcAddress, swapId);
 * // Use with viem:
 * await walletClient.sendTransaction({ to: refundData.to, data: refundData.data });
 * ```
 */
export function encodeRefundSwapCallData(
  htlcAddress: string,
  swapId: string,
): RefundSwapCallData {
  const swapIdEncoded = uuidToBytes32(swapId);
  const data = `${REFUND_SWAP_SELECTOR}${swapIdEncoded}`;

  return {
    to: htlcAddress,
    data,
    functionSignature: "refundSwap(bytes32)",
  };
}

/**
 * Parameters for HTLCErc20.refund function.
 */
export interface HtlcErc20RefundParams {
  preimageHash: string;
  amount: bigint | number;
  token: string;
  claimAddress: string;
  timelock: number;
}

/**
 * Result of encoding HTLCErc20.refund call data.
 */
export interface HtlcErc20RefundCallData {
  to: string;
  data: string;
  functionSignature: string;
}

/**
 * Encodes the call data for HTLCErc20.refund function.
 *
 * This is for refunding tokens from the HTLCErc20 contract after the timelock has expired.
 *
 * @param htlcAddress - The HTLCErc20 contract address
 * @param params - The refund parameters
 * @returns The encoded refund call data
 *
 * @example
 * ```ts
 * const refundData = encodeHtlcErc20RefundCallData(htlcAddress, {
 *   preimageHash: "0x...",
 *   amount: 100000000n,
 *   token: "0x...",
 *   claimAddress: "0x...",
 *   timelock: 1234567890,
 * });
 * // Use with viem:
 * await walletClient.sendTransaction({ to: refundData.to, data: refundData.data });
 * ```
 */
export function encodeHtlcErc20RefundCallData(
  htlcAddress: string,
  params: HtlcErc20RefundParams,
): HtlcErc20RefundCallData {
  const preimageHashEncoded = normalizeBytes32(params.preimageHash);
  const amountEncoded = encodeUint256(BigInt(params.amount));
  const tokenEncoded = normalizeAddress(params.token);
  const claimAddressEncoded = normalizeAddress(params.claimAddress);
  const timelockEncoded = encodeUint256(BigInt(params.timelock));

  const data = [
    HTLC_ERC20_REFUND_SELECTOR,
    preimageHashEncoded,
    amountEncoded,
    tokenEncoded,
    claimAddressEncoded,
    timelockEncoded,
  ].join("");

  return {
    to: htlcAddress,
    data,
    functionSignature: "refund(bytes32,uint256,address,address,uint256)",
  };
}

/** Parameters for `HTLCNative.refund` (no token: the lock is the native coin). */
export interface HtlcNativeRefundParams {
  preimageHash: string;
  amount: bigint | number;
  claimAddress: string;
  timelock: number;
}

// HTLCNative.refund(bytes32,uint256,address,uint256)
const HTLC_NATIVE_REFUND_SELECTOR = "0x35cd4ccb";

/**
 * Encodes `HTLCNative.refund(preimageHash, amount, claimAddress, timelock)`
 * for a lock created directly on the HTLC (sender == depositor). Locks made
 * through the native coordinator are refunded with `encodeNativeRefundTo`.
 */
export function encodeHtlcNativeRefundCallData(
  htlcAddress: string,
  params: HtlcNativeRefundParams,
): HtlcErc20RefundCallData {
  const data = [
    HTLC_NATIVE_REFUND_SELECTOR,
    normalizeBytes32(params.preimageHash),
    encodeUint256(BigInt(params.amount)),
    normalizeAddress(params.claimAddress),
    encodeUint256(BigInt(params.timelock)),
  ].join("");
  return {
    to: htlcAddress,
    data,
    functionSignature: "refund(bytes32,uint256,address,uint256)",
  };
}

/**
 * Whether client locks on this deployment are keyed under the depositor
 * rather than the coordinator: HTLCErc20 from v7, HTLCNative from v2. The
 * Permit2 funding witness binds `refundAddress` to that sender, so a funding
 * signed for the wrong one is rejected by Permit2 (`InvalidSigner`).
 * Unknown kind or version means an older server, whose deployments lock
 * with the coordinator.
 */
export function depositorIsSender(
  kind: string | undefined,
  version: number | undefined,
): boolean {
  if (version === undefined) return false;
  if (kind === "native") return version >= 2;
  if (kind === "erc20") return version >= 7;
  return false;
}

// HTLCErc20.refundFor(bytes32,uint256,address,address,address,uint256)
const HTLC_ERC20_REFUND_FOR_SELECTOR = "0x91fd66ab";
// HTLCNative.refundFor(bytes32,uint256,address,address,uint256)
const HTLC_NATIVE_REFUND_FOR_SELECTOR = "0x8dd4ea3e";

/**
 * Rewrites the server's permissionless `refundFor` of an expired lock
 * (HTLCErc20 v7 / HTLCNative v2) as the depositor's own `refund` on the same
 * HTLC, for a depositor that sends the refund itself, e.g. through a
 * sponsored UserOp, whose paymaster only sponsors a depositor's own calls.
 * Any other call is returned unchanged. Throws when the lock's sender is not
 * `depositor`, since `refund` would then compute a different key and revert.
 */
export function refundForAsOwnRefund(
  call: { to: string; data: string },
  depositor: string,
): { to: string; data: string } {
  const data = call.data.toLowerCase();
  const selector = data.slice(0, 10);
  const native = selector === HTLC_NATIVE_REFUND_FOR_SELECTOR;
  if (!native && selector !== HTLC_ERC20_REFUND_FOR_SELECTOR) return call;
  const words = native ? 5 : 6;
  if (data.length !== 10 + 64 * words) {
    throw new Error(
      `refundFor calldata has ${(data.length - 10) / 64} words, expected ${words}`,
    );
  }
  const word = (i: number) => data.slice(10 + 64 * i, 10 + 64 * (i + 1));
  const senderIndex = native ? 2 : 3;
  const sender = `0x${word(senderIndex).slice(24)}`;
  if (sender !== depositor.toLowerCase()) {
    throw new Error(
      `The lock was created by ${sender}, not ${depositor}; only its sender can refund it directly`,
    );
  }
  const terms = Array.from({ length: words }, (_, i) => word(i)).filter(
    (_, i) => i !== senderIndex,
  );
  const refundSelector = native
    ? HTLC_NATIVE_REFUND_SELECTOR
    : HTLC_ERC20_REFUND_SELECTOR;
  return { to: call.to, data: [refundSelector, ...terms].join("") };
}

/** A server signature over an HTLC-level EIP-712 struct, as `(v, r, s)`. */
export interface HtlcSignature {
  v: number;
  r: string;
  s: string;
}

/**
 * Parameters for `HTLCErc20.refundBySig`: the lock's terms plus the claim
 * address's `Refund` signature. The depositor is `msg.sender`, so the call
 * carries no sender or claim address; the contract recovers the latter.
 */
export interface HtlcErc20RefundBySigParams extends HtlcSignature {
  preimageHash: string;
  amount: bigint | number;
  token: string;
  timelock: number;
}

// HTLCErc20.refundBySig(bytes32,uint256,address,uint256,uint8,bytes32,bytes32)
const HTLC_ERC20_REFUND_BY_SIG_SELECTOR = "0x072c420d";

/**
 * Encodes `HTLCErc20.refundBySig(...)` (v7+): the depositor returns its lock
 * before the timelock with the claim address's `Refund` signature, which the
 * server hands out through `/collab-refund-evm/signature`. Must be sent from
 * the depositor address the lock was created under.
 */
export function encodeHtlcErc20RefundBySigCallData(
  htlcAddress: string,
  params: HtlcErc20RefundBySigParams,
): HtlcErc20RefundCallData {
  const data = [
    HTLC_ERC20_REFUND_BY_SIG_SELECTOR,
    normalizeBytes32(params.preimageHash),
    encodeUint256(BigInt(params.amount)),
    normalizeAddress(params.token),
    encodeUint256(BigInt(params.timelock)),
    ...encodeSignature(params),
  ].join("");
  return {
    to: htlcAddress,
    data,
    functionSignature:
      "refundBySig(bytes32,uint256,address,uint256,uint8,bytes32,bytes32)",
  };
}

/** Parameters for `HTLCNative.refundBySig` (no token: the lock is the native coin). */
export interface HtlcNativeRefundBySigParams extends HtlcSignature {
  preimageHash: string;
  amount: bigint | number;
  timelock: number;
}

// HTLCNative.refundBySig(bytes32,uint256,uint256,uint8,bytes32,bytes32)
const HTLC_NATIVE_REFUND_BY_SIG_SELECTOR = "0x3bab851e";

/** `HTLCNative.refundBySig(...)` (v2+); see {@link encodeHtlcErc20RefundBySigCallData}. */
export function encodeHtlcNativeRefundBySigCallData(
  htlcAddress: string,
  params: HtlcNativeRefundBySigParams,
): HtlcErc20RefundCallData {
  const data = [
    HTLC_NATIVE_REFUND_BY_SIG_SELECTOR,
    normalizeBytes32(params.preimageHash),
    encodeUint256(BigInt(params.amount)),
    encodeUint256(BigInt(params.timelock)),
    ...encodeSignature(params),
  ].join("");
  return {
    to: htlcAddress,
    data,
    functionSignature:
      "refundBySig(bytes32,uint256,uint256,uint8,bytes32,bytes32)",
  };
}

/**
 * Parameters for `HTLCErc20.rehash`: the lock's current terms, the hash and
 * timelock it continues under, and the claim address's `Rehash` signature.
 */
export interface HtlcErc20RehashParams extends HtlcSignature {
  preimageHash: string;
  amount: bigint | number;
  token: string;
  timelock: number;
  newPreimageHash: string;
  newTimelock: number;
}

// HTLCErc20.rehash(bytes32,uint256,address,uint256,bytes32,uint256,uint8,bytes32,bytes32)
const HTLC_ERC20_REHASH_SELECTOR = "0x5a83bf7f";

/**
 * Encodes `HTLCErc20.rehash(...)` (v7+): the depositor moves its active lock
 * to a new preimage hash and timelock with the claim address's `Rehash`
 * signature, which the server hands out through `/rehash-evm` together with
 * these arguments. Must be sent from the depositor address the lock was
 * created under.
 */
export function encodeHtlcErc20RehashCallData(
  htlcAddress: string,
  params: HtlcErc20RehashParams,
): HtlcErc20RefundCallData {
  const data = [
    HTLC_ERC20_REHASH_SELECTOR,
    normalizeBytes32(params.preimageHash),
    encodeUint256(BigInt(params.amount)),
    normalizeAddress(params.token),
    encodeUint256(BigInt(params.timelock)),
    normalizeBytes32(params.newPreimageHash),
    encodeUint256(BigInt(params.newTimelock)),
    ...encodeSignature(params),
  ].join("");
  return {
    to: htlcAddress,
    data,
    functionSignature:
      "rehash(bytes32,uint256,address,uint256,bytes32,uint256,uint8,bytes32,bytes32)",
  };
}

/** Parameters for `HTLCNative.rehash` (no token: the lock is the native coin). */
export interface HtlcNativeRehashParams extends HtlcSignature {
  preimageHash: string;
  amount: bigint | number;
  timelock: number;
  newPreimageHash: string;
  newTimelock: number;
}

// HTLCNative.rehash(bytes32,uint256,uint256,bytes32,uint256,uint8,bytes32,bytes32)
const HTLC_NATIVE_REHASH_SELECTOR = "0x6eecb3d8";

/** `HTLCNative.rehash(...)` (v2+); see {@link encodeHtlcErc20RehashCallData}. */
export function encodeHtlcNativeRehashCallData(
  htlcAddress: string,
  params: HtlcNativeRehashParams,
): HtlcErc20RefundCallData {
  const data = [
    HTLC_NATIVE_REHASH_SELECTOR,
    normalizeBytes32(params.preimageHash),
    encodeUint256(BigInt(params.amount)),
    encodeUint256(BigInt(params.timelock)),
    normalizeBytes32(params.newPreimageHash),
    encodeUint256(BigInt(params.newTimelock)),
    ...encodeSignature(params),
  ].join("");
  return {
    to: htlcAddress,
    data,
    functionSignature:
      "rehash(bytes32,uint256,uint256,bytes32,uint256,uint8,bytes32,bytes32)",
  };
}

/**
 * `(uint8 v, bytes32 r, bytes32 s)` as three ABI words. An `r` or `s` that
 * is not exactly 32 bytes is refused: padded or overlong hex would shift
 * every word after it instead of failing.
 */
function encodeSignature(sig: HtlcSignature): string[] {
  if (!Number.isInteger(sig.v) || sig.v < 0 || sig.v > 255) {
    throw new Error(`Signature v must be a uint8, got ${sig.v}`);
  }
  return [encodeUint256(BigInt(sig.v)), word("r", sig.r), word("s", sig.s)];
}

function word(name: string, value: string): string {
  const clean = value.replace(/^0x/, "");
  if (!/^[0-9a-fA-F]{64}$/.test(clean)) {
    throw new Error(`Signature ${name} must be 32 bytes of hex, got ${value}`);
  }
  return clean.toLowerCase();
}

export interface HtlcErc20IsActiveParams {
  preimageHash: string;
  amount: bigint;
  token: string;
  /** The HTLC's sender: whoever called `create`. */
  refundAddress: string;
  claimAddress: string;
  timelock: number;
}

/**
 * Encodes `HTLCErc20.isActive(...)`, which answers whether the swap keyed by
 * these parameters is still open (neither redeemed nor refunded). The result
 * is a single ABI word, non-zero for true.
 */
export function encodeHtlcErc20IsActiveCallData(
  htlcAddress: string,
  params: HtlcErc20IsActiveParams,
): { to: string; data: string } {
  const data = [
    HTLC_ERC20_IS_ACTIVE_SELECTOR,
    normalizeBytes32(params.preimageHash),
    encodeUint256(params.amount),
    normalizeAddress(params.token),
    normalizeAddress(params.refundAddress),
    normalizeAddress(params.claimAddress),
    encodeUint256(BigInt(params.timelock)),
  ].join("");
  return { to: htlcAddress, data };
}

/**
 * The key `HTLCErc20` / `HTLCNative` store a lock under:
 * `keccak256(abi.encode(preimageHash, amount, token, sender, claimAddress, timelock))`,
 * with a zero token word for a native lock.
 */
export function htlcSwapKey(params: HtlcErc20IsActiveParams): string {
  const encoded = [
    normalizeBytes32(params.preimageHash),
    encodeUint256(params.amount),
    normalizeAddress(params.token),
    normalizeAddress(params.refundAddress),
    normalizeAddress(params.claimAddress),
    encodeUint256(BigInt(params.timelock)),
  ].join("");
  return `0x${bytesToHex(keccak_256(hexToBytes(encoded)))}`;
}

// keccak256("swaps(bytes32)")[0:4]
const HTLC_SWAPS_SELECTOR = "0xeb84e7f2";

/** `SwapState` of HTLCErc20 v7 / HTLCNative v2. */
export const HTLC_SWAP_STATE = {
  None: 0n,
  Active: 1n,
  Redeemed: 2n,
  Refunded: 3n,
  Rehashed: 4n,
} as const;

/** Encodes `swaps(key)`, the lock's `SwapState` as one ABI word. */
export function encodeHtlcSwapsCallData(
  htlcAddress: string,
  key: string,
): { to: string; data: string } {
  return {
    to: htlcAddress,
    data: `${HTLC_SWAPS_SELECTOR}${normalizeBytes32(key)}`,
  };
}

/**
 * Encodes the call data for HTLCErc20.create function.
 *
 * This is for the new HTLCErc20 contract used in EVM-to-Lightning swaps.
 *
 * @param htlcAddress - The HTLCErc20 contract address
 * @param params - The create parameters
 * @returns The encoded create call data
 *
 * @example
 * ```ts
 * const createData = encodeHtlcErc20CreateCallData(htlcAddress, {
 *   preimageHash: "0x...",
 *   amount: 100000000n,
 *   token: "0x...",
 *   sender: "0x...",
 *   claimer: "0x...",
 *   timelock: Math.floor(Date.now() / 1000) + 3600,
 * });
 * // Use with viem:
 * await walletClient.sendTransaction({ to: createData.to, data: createData.data });
 * ```
 */
export function encodeHtlcErc20CreateCallData(
  htlcAddress: string,
  params: HtlcErc20CreateParams,
): HtlcErc20CreateCallData {
  const preimageHashEncoded = normalizeBytes32(params.preimageHash);
  const amountEncoded = encodeUint256(params.amount);
  const tokenEncoded = normalizeAddress(params.token);
  const senderEncoded = normalizeAddress(params.sender);
  const claimerEncoded = normalizeAddress(params.claimer);
  const timelockEncoded = encodeUint256(BigInt(params.timelock));

  const data = [
    HTLC_ERC20_CREATE_SELECTOR,
    preimageHashEncoded,
    amountEncoded,
    tokenEncoded,
    senderEncoded,
    claimerEncoded,
    timelockEncoded,
  ].join("");

  return {
    to: htlcAddress,
    data,
    functionSignature:
      "create(bytes32,uint256,address,address,address,uint256)",
  };
}

/**
 * Builds both approve and createSwap call data for an EVM-to-BTC swap.
 *
 * @param htlcAddress - The HTLC contract address
 * @param params - The createSwap parameters
 * @returns Both approve and createSwap call data
 *
 * @example
 * ```ts
 * const { approve, createSwap } = buildEvmHtlcCallData(htlcAddress, params);
 *
 * // Step 1: Approve token spend
 * await wallet.sendTransaction({ to: approve.to, data: approve.data });
 *
 * // Step 2: Create the swap
 * await wallet.sendTransaction({ to: createSwap.to, data: createSwap.data });
 * ```
 */
export function buildEvmHtlcCallData(
  htlcAddress: string,
  params: CreateSwapParams,
): {
  approve: ApproveCallData;
  createSwap: CreateSwapCallData;
} {
  return {
    approve: encodeApproveCallData(
      params.tokenIn,
      htlcAddress,
      params.amountIn,
    ),
    createSwap: encodeCreateSwapCallData(htlcAddress, params),
  };
}

// ── SwapCreated log ──────────────────────────────────────────────────────────

/** topic0 of `SwapCreated(bytes32,address,address,address,uint256,uint256,bytes32)`. */
export const SWAP_CREATED_TOPIC =
  "0x127ba86dffa89675eb78a2ca7203c29f865789ebec607b9424ed9f8d3d533e9f";

export interface SwapCreatedLog {
  preimageHash: string;
  /** The HTLC's sender; the coordinator for a Permit2-funded swap. */
  refundAddress: string;
  claimAddress: string;
  token: string;
  amount: bigint;
  timelock: number;
  key: string;
}

/**
 * Decodes an HTLCErc20 `SwapCreated` log, or returns undefined for any other
 * log. The amount here is the one the contract keys the swap on, which after
 * a DEX swap inside the funding tx can differ from every amount in the swap
 * record.
 */
export function decodeSwapCreatedLog(log: {
  topics: readonly string[];
  data: string;
}): SwapCreatedLog | undefined {
  if (log.topics.length !== 4) return undefined;
  if (log.topics[0].toLowerCase() !== SWAP_CREATED_TOPIC) return undefined;
  const data = log.data.replace(/^0x/, "");
  if (data.length !== 4 * 64) return undefined;
  const word = (i: number) => data.slice(i * 64, (i + 1) * 64);
  const address = (w: string) => `0x${w.slice(24)}`.toLowerCase();
  return {
    preimageHash: `0x${normalizeBytes32(log.topics[1])}`,
    refundAddress: address(normalizeBytes32(log.topics[2])),
    claimAddress: address(normalizeBytes32(log.topics[3])),
    token: address(word(0)),
    amount: decodeUint256(word(1)),
    timelock: Number(decodeUint256(word(2))),
    key: `0x${word(3)}`,
  };
}

// keccak256("SwapRehashed(bytes32,bytes32,bytes32,bytes32)")
export const SWAP_REHASHED_TOPIC =
  "0x54e7aaf61f7d94f49a761b306cc6ead18e2e3ce3fd40197cbd2cad8ff2aaf04a";

/** `SwapRehashed(preimageHash, key, newKey, newPreimageHash)`: a lock moved. */
export interface SwapRehashedLog {
  preimageHash: string;
  key: string;
  newKey: string;
  newPreimageHash: string;
}

/** Decodes an HTLC `SwapRehashed` log, or returns undefined for any other log. */
export function decodeSwapRehashedLog(log: {
  topics: readonly string[];
  data: string;
}): SwapRehashedLog | undefined {
  if (log.topics.length !== 4) return undefined;
  if (log.topics[0].toLowerCase() !== SWAP_REHASHED_TOPIC) return undefined;
  const data = log.data.replace(/^0x/, "");
  if (data.length !== 64) return undefined;
  return {
    preimageHash: `0x${normalizeBytes32(log.topics[1])}`,
    key: `0x${normalizeBytes32(log.topics[2])}`,
    newKey: `0x${normalizeBytes32(log.topics[3])}`,
    newPreimageHash: `0x${normalizeBytes32(data)}`,
  };
}

/** The `SwapRehashed` logs `htlcAddress` emitted for the lock keyed `key`. */
export function findSwapRehashed(
  logs: readonly ReceiptLog[],
  filter: { htlcAddress: string; key: string },
): SwapRehashedLog[] {
  const htlc = filter.htlcAddress.toLowerCase();
  const key = `0x${normalizeBytes32(filter.key)}`;
  const matches: SwapRehashedLog[] = [];
  for (const log of logs) {
    if (log.address.toLowerCase() !== htlc) continue;
    const rehashed = decodeSwapRehashedLog(log);
    if (rehashed === undefined || rehashed.key !== key) continue;
    matches.push(rehashed);
  }
  return matches;
}

export interface SwapCreatedFilter {
  htlcAddress: string;
  hashLock: string;
  claimAddress: string;
  /** Senders the HTLC may have been created by; any sender when omitted. */
  senders?: readonly string[];
  token?: string;
}

/**
 * The funding tx runs server-supplied DEX calls before the coordinator
 * creates the HTLC, so a receipt can carry decoys with the right hash lock;
 * the emitting contract and the sender are what tell them apart.
 */
export function findSwapCreated(
  logs: readonly ReceiptLog[],
  filter: SwapCreatedFilter,
): SwapCreatedLog[] {
  const htlc = filter.htlcAddress.toLowerCase();
  const hashLock = `0x${normalizeBytes32(filter.hashLock)}`;
  const claimAddress = filter.claimAddress.toLowerCase();
  const senders = filter.senders?.map((sender) => sender.toLowerCase());
  const token = filter.token?.toLowerCase();
  const matches: SwapCreatedLog[] = [];
  for (const log of logs) {
    if (log.address.toLowerCase() !== htlc) continue;
    const created = decodeSwapCreatedLog(log);
    if (
      created === undefined ||
      created.preimageHash !== hashLock ||
      created.claimAddress !== claimAddress ||
      (senders !== undefined && !senders.includes(created.refundAddress)) ||
      (token !== undefined && created.token !== token)
    ) {
      continue;
    }
    matches.push(created);
  }
  return matches;
}
