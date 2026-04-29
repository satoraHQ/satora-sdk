/**
 * CCTP utility functions.
 */

import { base58 } from "@scure/base";
import { CCTP_DOMAINS, type CctpChainName } from "./constants.js";

/**
 * Convert an EVM address to CCTP's bytes32 format (left-padded with zeros).
 * @param address - 0x-prefixed hex address (20 bytes)
 * @returns 0x-prefixed bytes32 hex string (32 bytes)
 */
export function addressToBytes32(address: string): string {
  const clean = address.toLowerCase().replace("0x", "");
  if (clean.length !== 40) {
    throw new Error(
      `Invalid address length: expected 40 hex chars, got ${clean.length}`,
    );
  }
  return `0x${clean.padStart(64, "0")}`;
}

/**
 * Convert a CCTP bytes32 back to an EVM address.
 * @param bytes32 - 0x-prefixed bytes32 hex string
 * @returns 0x-prefixed checksummed address
 */
export function bytes32ToAddress(bytes32: string): string {
  const clean = bytes32.replace("0x", "");
  if (clean.length !== 64) {
    throw new Error(
      `Invalid bytes32 length: expected 64 hex chars, got ${clean.length}`,
    );
  }
  return `0x${clean.slice(24)}`;
}

/**
 * Get the CCTP domain ID for a chain name.
 * @param chainName - Chain name (e.g. "Ethereum", "Polygon", "Arbitrum")
 * @returns The CCTP domain ID, or undefined if not supported.
 */
export function getDomain(chainName: string): number | undefined {
  return CCTP_DOMAINS[chainName as CctpChainName];
}

/**
 * Check if two chains require CCTP bridging (i.e., they're different chains).
 * @param sourceChain - Source chain name
 * @param targetChain - Target chain name
 * @returns true if CCTP bridging is needed
 */
export function needsBridge(sourceChain: string, targetChain: string): boolean {
  if (sourceChain === targetChain) return false;
  const sourceDomain = getDomain(sourceChain);
  const targetDomain = getDomain(targetChain);
  return sourceDomain !== undefined && targetDomain !== undefined;
}

/**
 * Validate a Solana address. Solana pubkeys are base58-encoded 32-byte
 * values, typically 32-44 base58 characters. Cheap structural check —
 * does not verify on-curve / system-program semantics.
 */
export function isValidSolanaAddress(address: string): boolean {
  if (!address) return false;
  if (address.length < 32 || address.length > 44) return false;
  // Disallow base58-illegal characters early so we don't rely on the
  // decode throwing for obvious junk.
  if (!/^[1-9A-HJ-NP-Za-km-z]+$/.test(address)) return false;
  try {
    return base58.decode(address).length === 32;
  } catch {
    return false;
  }
}

/**
 * Decode a base58-encoded Solana pubkey into CCTP's 32-byte recipient
 * format. Solana pubkeys are natively 32 bytes — no padding needed.
 *
 * Returns a `0x`-prefixed 64-character hex string (the bytes32 form
 * accepted by Circle's TokenMessenger `mintRecipient` field), so the
 * shape matches `addressToBytes32` for EVM addresses.
 */
export function solanaAddressToBytes32(address: string): string {
  const decoded = base58.decode(address);
  if (decoded.length !== 32) {
    throw new Error(
      `Solana pubkey must decode to 32 bytes, got ${decoded.length} for ${address}`,
    );
  }
  let hex = "0x";
  for (const byte of decoded) {
    hex += byte.toString(16).padStart(2, "0");
  }
  return hex;
}
