/**
 * Source-chain burn calldata for the CCTP-inbound flow.
 *
 * Encodes `TokenMessenger.depositForBurn(...)` on CCTPv2. The user submits
 * this tx on the source chain (e.g. Optimism, Base); the resulting CCTP
 * message lets our backend's Multicall3 tx on Arbitrum call
 * `receiveMessage` to mint USDC to the SDK-derived Arbitrum address.
 *
 * Note: we intentionally use the plain `depositForBurn` (no hook), with
 * `destinationCaller = bytes32(0)` so anyone can call `receiveMessage`.
 * The HTLC-locking logic lives on the Arbitrum side inside the multicall,
 * not in a CCTP hook.
 */

import { keccak_256 } from "@noble/hashes/sha3.js";
import { bytesToHex } from "@noble/hashes/utils.js";
import { FINALITY_FAST } from "../cctp/constants.js";
import { addressToBytes32 } from "../cctp/utils.js";

/** Parameters for `depositForBurn` on CCTPv2 TokenMessenger. */
export interface DepositForBurnParams {
  /** USDC amount in smallest units (6 decimals). */
  amount: bigint;
  /** CCTP destination domain ID (e.g. 3 = Arbitrum). */
  destinationDomain: number;
  /**
   * Destination address the USDC will be minted to. Accepts either a 20-byte
   * EVM address (will be left-padded to bytes32) or a pre-padded bytes32 hex.
   */
  mintRecipient: string;
  /** Source-chain USDC contract address. */
  burnToken: string;
  /**
   * Bytes32-encoded address allowed to call `receiveMessage` on destination.
   * Defaults to `bytes32(0)` — unrestricted, required for our backend-
   * submitted multicall flow.
   */
  destinationCaller?: string;
  /**
   * Maximum CCTPv2 fast-transfer fee in USDC units (from IRIS fee API).
   * Ignored if `minFinalityThreshold` is ≥ standard.
   */
  maxFee: bigint;
  /**
   * Finality threshold: `FINALITY_FAST` (1000) for fast transfers,
   * `FINALITY_STANDARD` (2000) for slow. Defaults to fast.
   */
  minFinalityThreshold?: number;
}

/**
 * Encodes the `depositForBurn` call data for CCTPv2 TokenMessenger.
 *
 * Returns a 0x-prefixed hex string ready to submit as a tx's `data` field
 * against the source chain's TokenMessenger address.
 */
export function encodeDepositForBurn(params: DepositForBurnParams): string {
  const destinationCaller =
    params.destinationCaller ??
    "0x0000000000000000000000000000000000000000000000000000000000000000";
  const minFinalityThreshold = params.minFinalityThreshold ?? FINALITY_FAST;

  // depositForBurn(uint256,uint32,bytes32,address,bytes32,uint256,uint32)
  const selector = keccak256Hex(
    "depositForBurn(uint256,uint32,bytes32,address,bytes32,uint256,uint32)",
  ).slice(0, 10);

  const mintRecipientBytes32 = ensureBytes32(params.mintRecipient);
  const destinationCallerBytes32 = ensureBytes32(destinationCaller);

  const encoded =
    selector +
    encodeUint256(params.amount) +
    encodeUint256(BigInt(params.destinationDomain)) +
    stripHex(mintRecipientBytes32) +
    encodeAddress(params.burnToken) +
    stripHex(destinationCallerBytes32) +
    encodeUint256(params.maxFee) +
    encodeUint256(BigInt(minFinalityThreshold));

  return encoded;
}

function ensureBytes32(value: string): string {
  const clean = value.replace(/^0x/, "");
  if (clean.length === 64) {
    return `0x${clean.toLowerCase()}`;
  }
  if (clean.length === 40) {
    return addressToBytes32(value);
  }
  throw new Error(
    `Invalid mintRecipient/destinationCaller: expected 20-byte address or 32-byte hex, got ${clean.length / 2} bytes`,
  );
}

function keccak256Hex(input: string): string {
  return `0x${bytesToHex(keccak_256(new TextEncoder().encode(input)))}`;
}

function encodeUint256(value: bigint): string {
  if (value < 0n) throw new Error("uint256 cannot be negative");
  return value.toString(16).padStart(64, "0");
}

function encodeAddress(address: string): string {
  return address.replace(/^0x/, "").toLowerCase().padStart(64, "0");
}

function stripHex(value: string): string {
  return value.replace(/^0x/, "").toLowerCase();
}
