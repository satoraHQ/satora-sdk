/**
 * HTLCCoordinator contract utilities.
 *
 * Provides helpers for EIP-712 signing and encoding `redeemAndExecute` call data
 * for the HTLCCoordinator contract used in Arkade-to-EVM swaps, and
 * `executeAndCreate` / refund helpers for EVM-to-BTC coordinator swaps.
 */

import { keccak_256 } from "@noble/hashes/sha3.js";
import {
  hexToBytes as nobleFromHex,
  bytesToHex as nobleToHex,
} from "@noble/hashes/utils.js";

// ── ABI helpers ──────────────────────────────────────────────────────────────

/** A single call struct for the coordinator's calls array: (address target, uint256 value, bytes data) */
export interface CoordinatorCall {
  /** Target contract address */
  target: string;
  /** ETH value to send (usually "0") */
  value: bigint;
  /** Encoded call data */
  data: string;
}

/** Parameters for building the EIP-712 redeem digest */
export interface RedeemDigestParams {
  /** HTLCErc20 contract address (verifyingContract) */
  htlcAddress: string;
  /** EVM chain ID */
  chainId: number;
  /** Preimage (32-byte hex with 0x prefix) */
  preimage: string;
  /** WBTC amount locked in the HTLC (in smallest unit) */
  amount: bigint;
  /** WBTC token address */
  token: string;
  /** HTLC sender (server's EVM address) */
  sender: string;
  /** HTLC timelock (unix timestamp) */
  timelock: number;
  /** Caller address (coordinator contract) */
  caller: string;
  /** Destination address where tokens are swept */
  destination: string;
  /** Token to sweep after calls (target token, or WBTC if no swap) */
  sweepToken: string;
  /** Minimum amount of sweepToken to receive (slippage protection) */
  minAmountOut: bigint;
  /** Hash of the calls array (prevents call substitution attacks) */
  callsHash: string;
}

/** Parameters for encoding redeemAndExecute call data */
export interface RedeemAndExecuteParams {
  /** Preimage (32-byte hex with 0x prefix) */
  preimage: string;
  /** WBTC amount locked in the HTLC */
  amount: bigint;
  /** WBTC token address */
  token: string;
  /** HTLC sender (server's EVM address) */
  sender: string;
  /** HTLC timelock */
  timelock: number;
  /** Array of calls to execute after redeem (approve + 1inch swap, or empty for WBTC) */
  calls: CoordinatorCall[];
  /** Token to sweep to the user after calls (target token, or WBTC if no swap) */
  sweepToken: string;
  /** Minimum amount of sweepToken to receive (slippage protection, 0 for no check) */
  minAmountOut: bigint;
  /** Destination address where tokens are swept */
  destination: string;
  /** EIP-712 signature v */
  v: number;
  /** EIP-712 signature r (32-byte hex with 0x prefix) */
  r: string;
  /** EIP-712 signature s (32-byte hex with 0x prefix) */
  s: string;
}

/** Result of building redeemAndExecute call data */
export interface RedeemAndExecuteCallData {
  /** The coordinator contract address */
  to: string;
  /** The encoded call data */
  data: string;
  /** Human-readable function signature */
  functionSignature: string;
}

/** Result of building coordinator call data (used by refund, Permit2, etc.) */
export interface ExecuteAndCreateCallData {
  /** The coordinator contract address */
  to: string;
  /** The encoded call data */
  data: string;
  /** Human-readable function signature */
  functionSignature: string;
}

/** Parameters for encoding refundAndExecute call data */
export interface RefundAndExecuteParams {
  /** SHA256 hash of the preimage (32-byte hex with 0x prefix) */
  preimageHash: string;
  /** WBTC amount locked in the HTLC */
  amount: bigint;
  /** WBTC token address */
  token: string;
  /** Claim address (server's EVM address) */
  claimAddress: string;
  /** HTLC timelock (unix timestamp) */
  timelock: number;
  /** Array of calls to execute (approve + reverse DEX swap) */
  calls: CoordinatorCall[];
  /** Token to sweep after calls (source token) */
  sweepToken: string;
  /** Minimum amount of sweepToken to receive (slippage protection) */
  minAmountOut: bigint;
}

/** Parameters for encoding refundTo call data */
export interface RefundToParams {
  /** SHA256 hash of the preimage (32-byte hex with 0x prefix) */
  preimageHash: string;
  /** WBTC amount locked in the HTLC */
  amount: bigint;
  /** WBTC token address */
  token: string;
  /** Claim address (server's EVM address) */
  claimAddress: string;
  /** HTLC timelock (unix timestamp) */
  timelock: number;
}

// ── EIP-712 constants ────────────────────────────────────────────────────────
// hardcoded so that we can potentially support multiple versions
const EIP712_DOMAIN_TYPEHASH =
  "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";
const REDEEM_TYPEHASH =
  "Redeem(bytes32 preimage,uint256 amount,address token,address sender,uint256 timelock,address caller,address destination,address sweepToken,uint256 minAmountOut,bytes32 callsHash)";
const HTLC_NAME = "HTLCErc20";
const HTLC_VERSION = "3";

// ── redeemAndExecute selector ────────────────────────────────────────────────
// keccak256("redeemAndExecute(bytes32,uint256,address,address,uint256,(address,uint256,bytes)[],address,uint256,address,uint8,bytes32,bytes32)")
const REDEEM_AND_EXECUTE_SELECTOR = keccak256(
  stringToUtf8Bytes(
    "redeemAndExecute(bytes32,uint256,address,address,uint256,(address,uint256,bytes)[],address,uint256,address,uint8,bytes32,bytes32)",
  ),
).slice(0, 10);

// ── keccak256 ────────────────────────────────────────────────────────────────

/**
 * Computes keccak256 hash using @noble/hashes.
 *
 * @param input - Hex string (with or without 0x) or Uint8Array
 * @returns 32-byte hex string with 0x prefix
 */
export function keccak256(input: string | Uint8Array): string {
  const data = typeof input === "string" ? hexToBytes(input) : input;
  const hash = keccak_256(data);
  return `0x${bytesToHex(hash)}`;
}

// ── EIP-712 digest ───────────────────────────────────────────────────────────

/**
 * Builds the EIP-712 digest that the user must sign to authorize
 * the coordinator to call `HTLC.redeem` on their behalf.
 *
 * @param params - The redeem parameters
 * @returns The 32-byte digest as hex string with 0x prefix
 *
 * @example
 * ```ts
 * const digest = buildRedeemDigest({
 *   htlcAddress: "0x...",
 *   chainId: 137,
 *   preimage: "0x...",
 *   amount: 100000n,
 *   token: "0x...", // WBTC
 *   sender: "0x...", // server
 *   timelock: 1700000000,
 *   caller: "0x...", // coordinator
 * });
 * // Sign `digest` with user's EVM wallet
 * ```
 */
export function buildRedeemDigest(params: RedeemDigestParams): string {
  // Domain separator
  const domainSeparator = keccak256(
    abiEncode([
      {
        type: "bytes32",
        value: keccak256(stringToUtf8Bytes(EIP712_DOMAIN_TYPEHASH)),
      },
      { type: "bytes32", value: keccak256(stringToUtf8Bytes(HTLC_NAME)) },
      { type: "bytes32", value: keccak256(stringToUtf8Bytes(HTLC_VERSION)) },
      { type: "uint256", value: BigInt(params.chainId) },
      { type: "address", value: params.htlcAddress },
    ]),
  );

  // Struct hash
  const typeHash = keccak256(stringToUtf8Bytes(REDEEM_TYPEHASH));
  const structHash = keccak256(
    abiEncode([
      { type: "bytes32", value: typeHash },
      { type: "bytes32", value: params.preimage },
      { type: "uint256", value: params.amount },
      { type: "address", value: params.token },
      { type: "address", value: params.sender },
      { type: "uint256", value: BigInt(params.timelock) },
      { type: "address", value: params.caller },
      { type: "address", value: params.destination },
      { type: "address", value: params.sweepToken },
      { type: "uint256", value: params.minAmountOut },
      { type: "bytes32", value: params.callsHash },
    ]),
  );

  // EIP-712 digest: \x19\x01 ‖ domainSeparator ‖ structHash
  const prefix = new Uint8Array([0x19, 0x01]);
  const domainBytes = hexToBytes(domainSeparator);
  const structBytes = hexToBytes(structHash);
  const message = new Uint8Array(
    prefix.length + domainBytes.length + structBytes.length,
  );
  message.set(prefix, 0);
  message.set(domainBytes, prefix.length);
  message.set(structBytes, prefix.length + domainBytes.length);

  return keccak256(message);
}

// ── Calls builder ────────────────────────────────────────────────────────────

/**
 * Builds the calls array for `redeemAndExecute` based on 1inch calldata.
 *
 * - If `dexCallData` is provided: returns [approve WBTC to DEX, execute DEX swap]
 * - If `dexCallData` is null/undefined (WBTC target): returns empty array
 *
 * @param wbtcAddress - WBTC token contract address
 * @param amount - WBTC amount to approve
 * @param dexCallData - DEX swap calldata from the creation response (optional)
 * @returns Array of CoordinatorCall structs
 */
export function buildRedeemCalls(
  wbtcAddress: string,
  amount: bigint,
  dexCallData?: { to: string; data: string; value: string } | null,
): CoordinatorCall[] {
  if (!dexCallData) {
    return [];
  }

  // Build approve calldata: WBTC.approve(dex_router, amount)
  const approveData = encodeApprove(dexCallData.to, amount);

  return [
    {
      target: wbtcAddress,
      value: 0n,
      data: approveData,
    },
    {
      target: dexCallData.to,
      value: BigInt(dexCallData.value || "0"),
      data: dexCallData.data,
    },
  ];
}

// ── redeemAndExecute calldata ────────────────────────────────────────────────

/**
 * Encodes the call data for `coordinator.redeemAndExecute(...)`.
 *
 * @param coordinatorAddress - The HTLCCoordinator contract address
 * @param params - All parameters for the call
 * @returns The encoded call data
 *
 * @example
 * ```ts
 * const txData = encodeRedeemAndExecute("0xCoordinator...", {
 *   preimage: "0x...",
 *   amount: 100000n,
 *   token: "0xWBTC...",
 *   sender: "0xServer...",
 *   timelock: 1700000000,
 *   calls: buildRedeemCalls(wbtcAddr, amount, dexCallData),
 *   sweepToken: targetTokenAddr,
 *   minAmountOut: 0n,
 *   v: 27,
 *   r: "0x...",
 *   s: "0x...",
 * });
 * // Send transaction: { to: txData.to, data: txData.data }
 * ```
 */
export function encodeRedeemAndExecute(
  coordinatorAddress: string,
  params: RedeemAndExecuteParams,
): RedeemAndExecuteCallData {
  // Fixed-length head: 12 slots of 32 bytes each
  // preimage (bytes32), amount (uint256), token (address), sender (address),
  // timelock (uint256), calls_offset (uint256), sweepToken (address),
  // minAmountOut (uint256), destination (address), v (uint8), r (bytes32), s (bytes32)
  const preimage = normalizeBytes32(params.preimage);
  const amount = encodeUint256(params.amount);
  const token = normalizeAddress(params.token);
  const sender = normalizeAddress(params.sender);
  const timelock = encodeUint256(BigInt(params.timelock));

  // Calls is a dynamic type — offset points to where the array data starts.
  // Head has 12 slots × 32 bytes = 384 = 0x180
  const callsOffset = encodeUint256(12n * 32n);

  const sweepToken = normalizeAddress(params.sweepToken);
  const minAmountOut = encodeUint256(params.minAmountOut);
  const destination = normalizeAddress(params.destination);
  const v = encodeUint256(BigInt(params.v));
  const r = normalizeBytes32(params.r);
  const s = normalizeBytes32(params.s);

  // Encode the calls array
  const callsEncoded = encodeCalls(params.calls);

  const data = [
    REDEEM_AND_EXECUTE_SELECTOR,
    preimage,
    amount,
    token,
    sender,
    timelock,
    callsOffset,
    sweepToken,
    minAmountOut,
    destination,
    v,
    r,
    s,
    callsEncoded,
  ].join("");

  return {
    to: coordinatorAddress,
    data,
    functionSignature:
      "redeemAndExecute(bytes32,uint256,address,address,uint256,(address,uint256,bytes)[],address,uint256,address,uint8,bytes32,bytes32)",
  };
}

// ── refundAndExecute selector ────────────────────────────────────────────────
// keccak256("refundAndExecute(bytes32,uint256,address,address,uint256,(address,uint256,bytes)[],address,uint256)")
const REFUND_AND_EXECUTE_SELECTOR = keccak256(
  stringToUtf8Bytes(
    "refundAndExecute(bytes32,uint256,address,address,uint256,(address,uint256,bytes)[],address,uint256)",
  ),
).slice(0, 10);

// ── refundTo selector ────────────────────────────────────────────────────────
// keccak256("refundTo(bytes32,uint256,address,address,uint256)")
const REFUND_TO_SELECTOR = keccak256(
  stringToUtf8Bytes("refundTo(bytes32,uint256,address,address,uint256)"),
).slice(0, 10);

/**
 * Encodes the call data for `coordinator.refundAndExecute(...)`.
 *
 * Signature: refundAndExecute(bytes32 preimageHash, uint256 amount, address token, address claimAddress, uint256 timelock, Call[] calls, address sweepToken, uint256 minAmountOut)
 *
 * @param coordinatorAddress - The HTLCCoordinator contract address
 * @param params - All parameters for the call
 * @returns The encoded call data
 */
export function encodeRefundAndExecute(
  coordinatorAddress: string,
  params: RefundAndExecuteParams,
): ExecuteAndCreateCallData {
  // Head: preimageHash, amount, token, claimAddress, timelock, calls_offset, sweepToken, minAmountOut (8 slots)
  const preimageHash = normalizeBytes32(params.preimageHash);
  const amount = encodeUint256(params.amount);
  const token = normalizeAddress(params.token);
  const claimAddress = normalizeAddress(params.claimAddress);
  const timelock = encodeUint256(BigInt(params.timelock));
  const callsOffset = encodeUint256(8n * 32n);
  const sweepToken = normalizeAddress(params.sweepToken);
  const minAmountOut = encodeUint256(params.minAmountOut);

  // Encode the calls array (tail)
  const callsEncoded = encodeCalls(params.calls);

  const data = [
    REFUND_AND_EXECUTE_SELECTOR,
    preimageHash,
    amount,
    token,
    claimAddress,
    timelock,
    callsOffset,
    sweepToken,
    minAmountOut,
    callsEncoded,
  ].join("");

  return {
    to: coordinatorAddress,
    data,
    functionSignature:
      "refundAndExecute(bytes32,uint256,address,address,uint256,(address,uint256,bytes)[],address,uint256)",
  };
}

/**
 * Encodes the call data for `coordinator.refundTo(...)`.
 *
 * Signature: refundTo(bytes32 preimageHash, uint256 amount, address token, address claimAddress, uint256 timelock)
 *
 * @param coordinatorAddress - The HTLCCoordinator contract address
 * @param params - All parameters for the call
 * @returns The encoded call data
 */
export function encodeRefundTo(
  coordinatorAddress: string,
  params: RefundToParams,
): ExecuteAndCreateCallData {
  const preimageHash = normalizeBytes32(params.preimageHash);
  const amount = encodeUint256(params.amount);
  const token = normalizeAddress(params.token);
  const claimAddress = normalizeAddress(params.claimAddress);
  const timelock = encodeUint256(BigInt(params.timelock));

  const data = [
    REFUND_TO_SELECTOR,
    preimageHash,
    amount,
    token,
    claimAddress,
    timelock,
  ].join("");

  return {
    to: coordinatorAddress,
    data,
    functionSignature: "refundTo(bytes32,uint256,address,address,uint256)",
  };
}

// ── Collaborative EVM refund EIP-712 ─────────────────────────────────────────

const COORDINATOR_NAME = "HTLCCoordinator";
const COORDINATOR_VERSION = "3";

const COLLAB_REFUND_TYPEHASH =
  "CollabRefund(bytes32 preimageHash,uint256 amount,address token,address claimAddress,uint256 timelock,address caller,address sweepToken,uint256 minAmountOut,bytes32 callsHash)";

/** Parameters for building the EIP-712 CollabRefund digest */
export interface CollabRefundEvmDigestParams {
  /** HTLCCoordinator contract address (verifyingContract) */
  coordinatorAddress: string;
  /** EVM chain ID */
  chainId: number;
  /** SHA-256 preimage hash (32-byte hex with 0x prefix) */
  preimageHash: string;
  /** WBTC amount locked in the HTLC (in smallest unit) */
  amount: bigint;
  /** WBTC token address */
  token: string;
  /** Claim address (server's EVM address) */
  claimAddress: string;
  /** HTLC timelock (unix timestamp) */
  timelock: number;
  /** Caller address (server EOA that submits the tx) */
  caller: string;
  /** Token to sweep after refund (WBTC for direct mode, source token for swap mode) */
  sweepToken: string;
  /** Minimum amount out (slippage protection, 0 for no check) */
  minAmountOut: bigint;
  /** keccak256(abi.encode(calls)) binding the coordinator calls array */
  callsHash: string;
}

/**
 * EIP-712 typed data structure for CollabRefund.
 *
 * Compatible with viem/wagmi's `signTypedData` and `eth_signTypedData_v4`.
 */
export interface CollabRefundEvmTypedData {
  domain: {
    name: string;
    version: string;
    chainId: number;
    verifyingContract: string;
  };
  types: {
    CollabRefund: Array<{ name: string; type: string }>;
  };
  primaryType: "CollabRefund";
  message: {
    preimageHash: string;
    amount: bigint;
    token: string;
    claimAddress: string;
    timelock: bigint;
    caller: string;
    sweepToken: string;
    minAmountOut: bigint;
    callsHash: string;
  };
}

/**
 * Builds the EIP-712 digest for collaborative EVM HTLC refund.
 *
 * The depositor signs this digest to authorize the server to submit
 * `collabRefundTo` or `collabRefundAndExecute` on the coordinator.
 *
 * @param params - CollabRefund parameters
 * @returns The 32-byte digest as hex string with 0x prefix
 */
export function buildCollabRefundEvmDigest(
  params: CollabRefundEvmDigestParams,
): string {
  // Coordinator domain separator
  const domainSeparator = keccak256(
    abiEncode([
      {
        type: "bytes32",
        value: keccak256(stringToUtf8Bytes(EIP712_DOMAIN_TYPEHASH)),
      },
      {
        type: "bytes32",
        value: keccak256(stringToUtf8Bytes(COORDINATOR_NAME)),
      },
      {
        type: "bytes32",
        value: keccak256(stringToUtf8Bytes(COORDINATOR_VERSION)),
      },
      { type: "uint256", value: BigInt(params.chainId) },
      { type: "address", value: params.coordinatorAddress },
    ]),
  );

  // CollabRefund struct hash
  const typeHash = keccak256(stringToUtf8Bytes(COLLAB_REFUND_TYPEHASH));
  const structHash = keccak256(
    abiEncode([
      { type: "bytes32", value: typeHash },
      { type: "bytes32", value: params.preimageHash },
      { type: "uint256", value: params.amount },
      { type: "address", value: params.token },
      { type: "address", value: params.claimAddress },
      { type: "uint256", value: BigInt(params.timelock) },
      { type: "address", value: params.caller },
      { type: "address", value: params.sweepToken },
      { type: "uint256", value: params.minAmountOut },
      { type: "bytes32", value: params.callsHash },
    ]),
  );

  // EIP-712 digest: \x19\x01 ‖ domainSeparator ‖ structHash
  const prefix = new Uint8Array([0x19, 0x01]);
  const domainBytes = hexToBytes(domainSeparator);
  const structBytes = hexToBytes(structHash);
  const message = new Uint8Array(
    prefix.length + domainBytes.length + structBytes.length,
  );
  message.set(prefix, 0);
  message.set(domainBytes, prefix.length);
  message.set(structBytes, prefix.length + domainBytes.length);

  return keccak256(message);
}

/**
 * Builds the EIP-712 typed data structure for CollabRefund.
 *
 * For use with browser wallets via `eth_signTypedData_v4` / wagmi's `signTypedData`.
 *
 * @param params - CollabRefund parameters
 * @returns EIP-712 typed data compatible with viem/wagmi
 */
export function buildCollabRefundEvmTypedData(
  params: CollabRefundEvmDigestParams,
): CollabRefundEvmTypedData {
  return {
    domain: {
      name: COORDINATOR_NAME,
      version: COORDINATOR_VERSION,
      chainId: params.chainId,
      verifyingContract: params.coordinatorAddress,
    },
    types: {
      CollabRefund: [
        { name: "preimageHash", type: "bytes32" },
        { name: "amount", type: "uint256" },
        { name: "token", type: "address" },
        { name: "claimAddress", type: "address" },
        { name: "timelock", type: "uint256" },
        { name: "caller", type: "address" },
        { name: "sweepToken", type: "address" },
        { name: "minAmountOut", type: "uint256" },
        { name: "callsHash", type: "bytes32" },
      ],
    },
    primaryType: "CollabRefund",
    message: {
      preimageHash: params.preimageHash,
      amount: params.amount,
      token: params.token,
      claimAddress: params.claimAddress,
      timelock: BigInt(params.timelock),
      caller: params.caller,
      sweepToken: params.sweepToken,
      minAmountOut: params.minAmountOut,
      callsHash: params.callsHash,
    },
  };
}

// ── Permit2 constants ─────────────────────────────────────────────────────────

/** Canonical Permit2 deployment address (same on all EVM chains) */
export const PERMIT2_ADDRESS = "0x000000000022D473030F116dDEE9F6B43aC78BA3";

// Permit2 uses a simpler domain: EIP712Domain(string name, uint256 chainId, address verifyingContract)
const PERMIT2_DOMAIN_TYPEHASH =
  "EIP712Domain(string name,uint256 chainId,address verifyingContract)";

const PERMIT2_NAME = "Permit2";

// Full type string for permitWitnessTransferFrom — includes sub-types alphabetically
const PERMIT_WITNESS_TRANSFER_FROM_TYPEHASH =
  "PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,ExecuteAndCreate witness)" +
  "ExecuteAndCreate(bytes32 preimageHash,address token,address claimAddress,address refundAddress,uint256 timelock,bytes32 callsHash)" +
  "TokenPermissions(address token,uint256 amount)";

const TOKEN_PERMISSIONS_TYPEHASH =
  "TokenPermissions(address token,uint256 amount)";

const EXECUTE_AND_CREATE_WITNESS_TYPEHASH =
  "ExecuteAndCreate(bytes32 preimageHash,address token,address claimAddress,address refundAddress,uint256 timelock,bytes32 callsHash)";

// ── Permit2 interfaces ───────────────────────────────────────────────────────

/** Parameters for building the Permit2 EIP-712 digest */
export interface Permit2FundingParams {
  /** EVM chain ID */
  chainId: number;
  /** HTLCCoordinator contract address (the spender) */
  coordinatorAddress: string;
  /** Source token address (what user holds, e.g. USDC) */
  sourceToken: string;
  /** Source amount in smallest units */
  sourceAmount: bigint;
  /** SHA-256 preimage hash (0x-prefixed, 32 bytes) */
  preimageHash: string;
  /** Lock token address (WBTC — the token field in the witness) */
  lockToken: string;
  /** Server's claim address */
  claimAddress: string;
  /** Refund address — coordinator address for overload 1 (depositor tracking) */
  refundAddress: string;
  /** HTLC timelock (unix timestamp) */
  timelock: number;
  /** keccak256(abi.encode(calls)) — binds the calls array in the witness */
  callsHash: string;
  /** Random Permit2 nonce (one-use) */
  nonce: bigint;
  /** Signature expiry timestamp */
  deadline: bigint;
}

/** Result of building Permit2-based coordinator funding call data */
export interface Permit2SignedFundingCallData {
  /** One-time approve: source token → Permit2 (max uint256) */
  approve: { to: string; data: string };
  /** The executeAndCreateWithPermit2 tx */
  executeAndCreate: { to: string; data: string };
}

/**
 * EIP-712 typed data structure for Permit2 permitWitnessTransferFrom.
 *
 * Compatible with viem/wagmi's `signTypedData` and `eth_signTypedData_v4`.
 * Used in the sovereign flow where the user's browser wallet signs
 * the Permit2 message directly.
 */
export interface Permit2TypedData {
  domain: {
    name: string;
    chainId: number;
    verifyingContract: string;
  };
  types: {
    PermitWitnessTransferFrom: Array<{ name: string; type: string }>;
    TokenPermissions: Array<{ name: string; type: string }>;
    ExecuteAndCreate: Array<{ name: string; type: string }>;
  };
  primaryType: "PermitWitnessTransferFrom";
  message: {
    permitted: { token: string; amount: bigint };
    spender: string;
    nonce: bigint;
    deadline: bigint;
    witness: {
      preimageHash: string;
      token: string;
      claimAddress: string;
      refundAddress: string;
      timelock: bigint;
      callsHash: string;
    };
  };
}

/** Unsigned Permit2 funding data returned by `getPermit2FundingParamsUnsigned`. */
export interface UnsignedPermit2FundingData {
  /** Coordinator contract address */
  coordinatorAddress: string;
  /** Source token address */
  sourceTokenAddress: string;
  /** Source amount in smallest units */
  sourceAmount: bigint;
  /** Lock token address (WBTC) */
  lockTokenAddress: string;
  /** Preimage hash */
  preimageHash: string;
  /** Server's claim address */
  claimAddress: string;
  /** HTLC timelock */
  timelock: number;
  /** Calls array for the coordinator */
  calls: CoordinatorCall[];
  /** Calls hash */
  callsHash: string;
  /** Random Permit2 nonce */
  nonce: bigint;
  /** Signature deadline */
  deadline: bigint;
  /** EIP-712 typed data for wallet signing (pass to signTypedData) */
  typedData: Permit2TypedData;
}

// ── Permit2 EIP-712 digest ───────────────────────────────────────────────────

/**
 * Builds the EIP-712 digest for Permit2 `permitWitnessTransferFrom` with
 * an `ExecuteAndCreate` witness.
 *
 * The user signs this digest to authorize the coordinator to pull their
 * source tokens via Permit2 and execute the swap + HTLC creation.
 *
 * @param params - The Permit2 funding parameters
 * @returns The 32-byte digest as hex string with 0x prefix
 */
export function buildPermit2FundingDigest(
  params: Permit2FundingParams,
): string {
  // 1. Permit2 domain separator (simpler than standard EIP-712 — no version)
  const domainSeparator = keccak256(
    abiEncode([
      {
        type: "bytes32",
        value: keccak256(stringToUtf8Bytes(PERMIT2_DOMAIN_TYPEHASH)),
      },
      { type: "bytes32", value: keccak256(stringToUtf8Bytes(PERMIT2_NAME)) },
      { type: "uint256", value: BigInt(params.chainId) },
      { type: "address", value: PERMIT2_ADDRESS },
    ]),
  );

  // 2. TokenPermissions hash
  const tokenPermissionsHash = keccak256(
    abiEncode([
      {
        type: "bytes32",
        value: keccak256(stringToUtf8Bytes(TOKEN_PERMISSIONS_TYPEHASH)),
      },
      { type: "address", value: params.sourceToken },
      { type: "uint256", value: params.sourceAmount },
    ]),
  );

  // 3. ExecuteAndCreate witness hash
  const witnessHash = keccak256(
    abiEncode([
      {
        type: "bytes32",
        value: keccak256(
          stringToUtf8Bytes(EXECUTE_AND_CREATE_WITNESS_TYPEHASH),
        ),
      },
      { type: "bytes32", value: params.preimageHash },
      { type: "address", value: params.lockToken },
      { type: "address", value: params.claimAddress },
      { type: "address", value: params.refundAddress },
      { type: "uint256", value: BigInt(params.timelock) },
      { type: "bytes32", value: params.callsHash },
    ]),
  );

  // 4. Struct hash: PermitWitnessTransferFrom(tokenPermissionsHash, spender, nonce, deadline, witnessHash)
  const structHash = keccak256(
    abiEncode([
      {
        type: "bytes32",
        value: keccak256(
          stringToUtf8Bytes(PERMIT_WITNESS_TRANSFER_FROM_TYPEHASH),
        ),
      },
      { type: "bytes32", value: tokenPermissionsHash },
      { type: "address", value: params.coordinatorAddress },
      { type: "uint256", value: params.nonce },
      { type: "uint256", value: params.deadline },
      { type: "bytes32", value: witnessHash },
    ]),
  );

  // 5. EIP-712 digest: \x19\x01 ‖ domainSeparator ‖ structHash
  const prefix = new Uint8Array([0x19, 0x01]);
  const domainBytes = hexToBytes(domainSeparator);
  const structBytes = hexToBytes(structHash);
  const message = new Uint8Array(
    prefix.length + domainBytes.length + structBytes.length,
  );
  message.set(prefix, 0);
  message.set(domainBytes, prefix.length);
  message.set(structBytes, prefix.length + domainBytes.length);

  return keccak256(message);
}

// ── Permit2 typed data for wallet signing ──────────────────────────────────

/**
 * Builds the EIP-712 typed data structure for Permit2 `permitWitnessTransferFrom`
 * with an `ExecuteAndCreate` witness.
 *
 * This is intended for the "sovereign" flow where the user's browser wallet
 * signs the Permit2 message via `eth_signTypedData_v4` / wagmi's `signTypedData`.
 *
 * @param params - The Permit2 funding parameters
 * @returns EIP-712 typed data compatible with viem/wagmi's `signTypedData`
 *
 * @example
 * ```ts
 * const typedData = buildPermit2TypedData(params);
 * // In the browser with wagmi:
 * const signature = await walletClient.signTypedData(typedData);
 * ```
 */
export function buildPermit2TypedData(
  params: Permit2FundingParams,
): Permit2TypedData {
  return {
    domain: {
      name: PERMIT2_NAME,
      chainId: params.chainId,
      verifyingContract: PERMIT2_ADDRESS,
    },
    types: {
      PermitWitnessTransferFrom: [
        { name: "permitted", type: "TokenPermissions" },
        { name: "spender", type: "address" },
        { name: "nonce", type: "uint256" },
        { name: "deadline", type: "uint256" },
        { name: "witness", type: "ExecuteAndCreate" },
      ],
      TokenPermissions: [
        { name: "token", type: "address" },
        { name: "amount", type: "uint256" },
      ],
      ExecuteAndCreate: [
        { name: "preimageHash", type: "bytes32" },
        { name: "token", type: "address" },
        { name: "claimAddress", type: "address" },
        { name: "refundAddress", type: "address" },
        { name: "timelock", type: "uint256" },
        { name: "callsHash", type: "bytes32" },
      ],
    },
    primaryType: "PermitWitnessTransferFrom",
    message: {
      permitted: {
        token: params.sourceToken,
        amount: params.sourceAmount,
      },
      spender: params.coordinatorAddress,
      nonce: params.nonce,
      deadline: params.deadline,
      witness: {
        preimageHash: params.preimageHash,
        token: params.lockToken,
        claimAddress: params.claimAddress,
        refundAddress: params.refundAddress,
        timelock: BigInt(params.timelock),
        callsHash: params.callsHash,
      },
    },
  };
}

// ── executeAndCreateWithPermit2 selector (overload 1: depositor tracking) ────
// keccak256("executeAndCreateWithPermit2((address,uint256,bytes)[],bytes32,address,address,uint256,address,((address,uint256),uint256,uint256),bytes)")
const EXECUTE_AND_CREATE_WITH_PERMIT2_SELECTOR = keccak256(
  stringToUtf8Bytes(
    "executeAndCreateWithPermit2((address,uint256,bytes)[],bytes32,address,address,uint256,address,((address,uint256),uint256,uint256),bytes)",
  ),
).slice(0, 10);

// ── executeAndCreateWithPermit2 calldata ─────────────────────────────────────

/** Parameters for encoding executeAndCreateWithPermit2 call data */
export interface ExecuteAndCreateWithPermit2Params {
  /** Array of calls to execute (approve DEX + DEX swap) */
  calls: CoordinatorCall[];
  /** SHA-256 preimage hash (32-byte hex with 0x prefix) */
  preimageHash: string;
  /** Lock token address (WBTC) */
  token: string;
  /** Server's claim address */
  claimAddress: string;
  /** HTLC timelock (unix timestamp) */
  timelock: number;
  /** Depositor address (the user whose tokens are pulled via Permit2) */
  depositor: string;
  /** Source token address (permitted token in the Permit2 struct) */
  sourceToken: string;
  /** Source amount (permitted amount) */
  sourceAmount: bigint;
  /** Permit2 nonce */
  nonce: bigint;
  /** Permit2 deadline */
  deadline: bigint;
  /** Signature bytes (compact 65-byte: r || s || v) */
  signature: string;
}

/**
 * Encodes the call data for `coordinator.executeAndCreateWithPermit2(...)` (overload 1).
 *
 * Overload 1 uses depositor tracking: the coordinator becomes the HTLC sender,
 * enabling server-side refundTo/refundAndExecute for expired swaps.
 *
 * Signature: executeAndCreateWithPermit2(
 *   Call[] calls,
 *   bytes32 preimageHash,
 *   address token,
 *   address claimAddress,
 *   uint256 timelock,
 *   address depositor,
 *   ((address,uint256),uint256,uint256) permit,
 *   bytes signature
 * )
 *
 * @param coordinatorAddress - The HTLCCoordinator contract address
 * @param params - All parameters for the call
 * @returns The encoded call data
 */
export function encodeExecuteAndCreateWithPermit2(
  coordinatorAddress: string,
  params: ExecuteAndCreateWithPermit2Params,
): ExecuteAndCreateCallData {
  // ABI head layout (11 words):
  //   0:  calls offset (dynamic → pointer)
  //   1:  preimageHash (bytes32)
  //   2:  token (address)
  //   3:  claimAddress (address)
  //   4:  timelock (uint256)
  //   5:  depositor (address)
  //   6:  permit.permitted.token (address)     ← inline static struct
  //   7:  permit.permitted.amount (uint256)    ← inline
  //   8:  permit.nonce (uint256)               ← inline
  //   9:  permit.deadline (uint256)            ← inline
  //   10: signature offset (dynamic → pointer)
  //
  // Tail:
  //   calls data
  //   signature data (length + padded bytes)
  const HEAD_WORDS = 11n;

  const preimageHash = normalizeBytes32(params.preimageHash);
  const token = normalizeAddress(params.token);
  const claimAddress = normalizeAddress(params.claimAddress);
  const timelock = encodeUint256(BigInt(params.timelock));
  const depositor = normalizeAddress(params.depositor);

  // Permit struct fields (encoded inline in the head)
  const permitToken = normalizeAddress(params.sourceToken);
  const permitAmount = encodeUint256(params.sourceAmount);
  const permitNonce = encodeUint256(params.nonce);
  const permitDeadline = encodeUint256(params.deadline);

  // Encode the calls array (tail)
  const callsEncoded = encodeCalls(params.calls);
  const callsEncodedBytes = BigInt(callsEncoded.length / 2);

  // Encode signature as dynamic bytes (tail)
  const sigClean = params.signature.startsWith("0x")
    ? params.signature.slice(2)
    : params.signature;
  const sigLength = sigClean.length / 2;
  const sigPaddedLength = Math.ceil(sigLength / 32) * 32;
  const signatureEncoded =
    encodeUint256(BigInt(sigLength)) +
    sigClean.padEnd(sigPaddedLength * 2, "0");

  // Compute offsets (relative to start of params, after selector)
  const callsOffset = HEAD_WORDS * 32n;
  const signatureOffset = callsOffset + callsEncodedBytes;

  const data = [
    EXECUTE_AND_CREATE_WITH_PERMIT2_SELECTOR,
    encodeUint256(callsOffset), // 0: calls offset
    preimageHash, // 1: preimageHash
    token, // 2: token
    claimAddress, // 3: claimAddress
    timelock, // 4: timelock
    depositor, // 5: depositor
    permitToken, // 6: permit.permitted.token
    permitAmount, // 7: permit.permitted.amount
    permitNonce, // 8: permit.nonce
    permitDeadline, // 9: permit.deadline
    encodeUint256(signatureOffset), // 10: signature offset
    callsEncoded, // tail: calls data
    signatureEncoded, // tail: signature data
  ].join("");

  return {
    to: coordinatorAddress,
    data,
    functionSignature:
      "executeAndCreateWithPermit2((address,uint256,bytes)[],bytes32,address,address,uint256,address,((address,uint256),uint256,uint256),bytes)",
  };
}

// ── EIP-2612 Permit digest ───────────────────────────────────────────────────

/** Parameters for building the EIP-2612 permit digest */
export interface Eip2612PermitParams {
  /** Token's EIP-712 domain separator (0x-prefixed, from token.DOMAIN_SEPARATOR()) */
  domainSeparator: string;
  /** Token owner address */
  owner: string;
  /** Spender address (typically Permit2) */
  spender: string;
  /** Approval amount */
  value: bigint;
  /** Token nonce for the owner (from token.nonces(owner)) */
  nonce: number;
  /** Signature deadline (unix timestamp) */
  deadline: bigint;
}

/**
 * Builds the EIP-712 digest for an EIP-2612 token permit.
 *
 * This is the standard ERC-2612 permit signature that allows gasless
 * token approvals. The user signs this to let `spender` (Permit2) spend
 * their tokens without an on-chain approve transaction.
 *
 * @param params - The EIP-2612 permit parameters
 * @returns The 32-byte digest as hex string with 0x prefix
 */
export function buildEip2612PermitDigest(params: Eip2612PermitParams): string {
  const PERMIT_TYPEHASH = keccak256(
    stringToUtf8Bytes(
      "Permit(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)",
    ),
  );
  const structHash = keccak256(
    abiEncode([
      { type: "bytes32", value: PERMIT_TYPEHASH },
      { type: "address", value: params.owner },
      { type: "address", value: params.spender },
      { type: "uint256", value: params.value },
      { type: "uint256", value: BigInt(params.nonce) },
      { type: "uint256", value: params.deadline },
    ]),
  );

  // EIP-712 digest: 0x1901 || domainSeparator || structHash
  const prefix = new Uint8Array([0x19, 0x01]);
  const domainBytes = hexToBytes(params.domainSeparator);
  const structBytes = hexToBytes(structHash);
  const message = new Uint8Array(
    prefix.length + domainBytes.length + structBytes.length,
  );
  message.set(prefix, 0);
  message.set(domainBytes, prefix.length);
  message.set(structBytes, prefix.length + domainBytes.length);

  return keccak256(message);
}

// ── Internal encoding helpers ────────────────────────────────────────────────

/** Encode ERC20 approve(address,uint256) call data */
function encodeApprove(spender: string, amount: bigint): string {
  const selector = "0x095ea7b3";
  return `${selector}${normalizeAddress(spender)}${encodeUint256(amount)}`;
}

/** Encode the dynamic (address,uint256,bytes)[] array for ABI */
function encodeCalls(calls: CoordinatorCall[]): string {
  // Array length
  const length = encodeUint256(BigInt(calls.length));

  if (calls.length === 0) {
    return length;
  }

  // Each element is a tuple (address, uint256, bytes) — a dynamic type.
  // We encode offsets first, then each element's data.

  // Calculate offsets: each element offset is relative to the start of the array data
  // (after the length word). First we have N offset words, then the actual data.
  const elementDataParts: string[] = [];
  const offsets: bigint[] = [];

  // First pass: encode each element and compute sizes
  for (const call of calls) {
    const encoded = encodeSingleCall(call);
    elementDataParts.push(encoded);
  }

  // Compute offsets: offset[0] = N * 32, offset[i] = offset[i-1] + size(element[i-1])
  let currentOffset = BigInt(calls.length) * 32n;
  for (let i = 0; i < calls.length; i++) {
    offsets.push(currentOffset);
    // Each encoded element is hex chars / 2 = bytes
    currentOffset += BigInt(elementDataParts[i].length / 2);
  }

  const offsetsEncoded = offsets.map((o) => encodeUint256(o)).join("");
  const dataEncoded = elementDataParts.join("");

  return length + offsetsEncoded + dataEncoded;
}

/** Encode a single (address, uint256, bytes) tuple */
function encodeSingleCall(call: CoordinatorCall): string {
  const target = normalizeAddress(call.target);
  const value = encodeUint256(call.value);

  // bytes is dynamic: offset (32 bytes) + length (32 bytes) + padded data
  const bytesOffset = encodeUint256(3n * 32n); // offset after target, value, and this offset word

  const callData = call.data.startsWith("0x") ? call.data.slice(2) : call.data;
  const dataLength = callData.length / 2;
  const bytesLength = encodeUint256(BigInt(dataLength));

  // Pad data to 32-byte boundary
  const paddedLength = Math.ceil(dataLength / 32) * 32;
  const paddedData = callData.padEnd(paddedLength * 2, "0");

  return target + value + bytesOffset + bytesLength + paddedData;
}

// ── Low-level ABI encoding ───────────────────────────────────────────────────

interface AbiValue {
  type: "bytes32" | "uint256" | "address";
  value: string | bigint;
}

function abiEncode(values: AbiValue[]): string {
  return values
    .map((v) => {
      switch (v.type) {
        case "bytes32":
          return normalizeBytes32(v.value as string);
        case "uint256":
          return encodeUint256(v.value as bigint);
        case "address":
          return normalizeAddress(v.value as string);
        default:
          throw new Error(`Unknown ABI type: ${v.type}`);
      }
    })
    .join("");
}

function normalizeBytes32(value: string): string {
  let clean = value.replace(/^0x/, "");
  if (clean.length < 64) {
    clean = clean.padStart(64, "0");
  }
  return clean.toLowerCase().slice(0, 64);
}

function normalizeAddress(address: string): string {
  const clean = address.replace(/^0x/, "").toLowerCase();
  return clean.padStart(64, "0");
}

function encodeUint256(value: bigint): string {
  return value.toString(16).padStart(64, "0");
}

// ── Hex / bytes utilities ────────────────────────────────────────────────────

function hexToBytes(hex: string): Uint8Array {
  return nobleFromHex(hex.replace(/^0x/, ""));
}

function bytesToHex(bytes: Uint8Array): string {
  return nobleToHex(bytes);
}

function stringToUtf8Bytes(str: string): Uint8Array {
  return new TextEncoder().encode(str);
}
