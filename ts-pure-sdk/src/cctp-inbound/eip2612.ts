/**
 * EIP-2612 permit signing for USDC → Permit2 approval using the SDK's
 * HD-derived EVM key.
 *
 * On a brand-new SDK-derived Arbitrum address, USDC's allowance for Permit2
 * is zero. Rather than doing an on-chain `approve` (which requires ETH the
 * user doesn't have), we sign an EIP-2612 `permit` off-chain; the backend's
 * multicall on Arbitrum includes `USDC.permit(...)` to establish the
 * allowance atomically before `executeAndCreateWithPermit2` pulls via Permit2.
 */

import { keccak_256 } from "@noble/hashes/sha3.js";
import { bytesToHex } from "@noble/hashes/utils.js";
import {
  buildEip2612PermitDigest,
  type Eip2612PermitParams,
  PERMIT2_ADDRESS,
} from "../evm/coordinator.js";
import { signEvmDigest } from "../evm/signing.js";

/** Parameters for signing an EIP-2612 permit against a specific token. */
export interface SignEip2612Params {
  /** Signer's secp256k1 private key (from the SDK HD signer). */
  secretKey: Uint8Array | string;
  /** The token's EIP-712 DOMAIN_SEPARATOR (0x-prefixed, from `token.DOMAIN_SEPARATOR()`). */
  domainSeparator: string;
  /** Token owner address (= `deriveEvmAddress(secretKey)`). */
  owner: string;
  /**
   * Spender. Defaults to Permit2 (the usual target — this is what enables
   * `executeAndCreateWithPermit2` to pull via Permit2 in the same multicall).
   */
  spender?: string;
  /** Approval amount. Pass `MAX_UINT256` to remove the need for future permits. */
  value: bigint;
  /** Current EIP-2612 nonce for `owner` on the token (from `token.nonces(owner)`). */
  nonce: number;
  /** Signature deadline (unix seconds). */
  deadline: bigint;
}

/** Signed EIP-2612 permit ready to submit on-chain. */
export interface SignedEip2612Permit {
  /** v, r, s components — v is 27 or 28. */
  v: number;
  r: string;
  s: string;
  /** Approval amount (as stringified decimal for API transport). */
  value: string;
  /** Signature deadline (unix seconds). */
  deadline: number;
}

/** uint256 max — sign once, never need another permit. */
export const MAX_UINT256 = (1n << 256n) - 1n;

/**
 * Signs an EIP-2612 permit using the SDK's derived key, returning the
 * split signature + value/deadline in a shape the backend's gasless
 * endpoint accepts directly.
 */
export function signEip2612Permit(
  params: SignEip2612Params,
): SignedEip2612Permit {
  const spender = params.spender ?? PERMIT2_ADDRESS;

  const digestParams: Eip2612PermitParams = {
    domainSeparator: params.domainSeparator,
    owner: params.owner,
    spender,
    value: params.value,
    nonce: params.nonce,
    deadline: params.deadline,
  };
  const digest = buildEip2612PermitDigest(digestParams);
  const sig = signEvmDigest(params.secretKey, digest);

  return {
    v: sig.v,
    r: sig.r,
    s: sig.s,
    value: params.value.toString(),
    deadline: Number(params.deadline),
  };
}

// ── Domain separator helper ──────────────────────────────────────────────────

/**
 * Builds an EIP-712 domain separator from the standard 4-field domain.
 *
 * Use when you know the token's on-chain `name` and `version` and want to
 * avoid an RPC call to `token.DOMAIN_SEPARATOR()`. For USDC deployments,
 * both vary slightly — see {@link USDC_DOMAIN_FIELDS}.
 */
export function buildDomainSeparator(params: {
  name: string;
  version: string;
  chainId: number;
  verifyingContract: string;
}): string {
  const TYPEHASH = keccak256Hex(
    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
  );
  const encoded =
    strip(TYPEHASH) +
    strip(keccak256Hex(params.name)) +
    strip(keccak256Hex(params.version)) +
    uint256Hex(BigInt(params.chainId)) +
    addressHex(params.verifyingContract);

  return keccak256HexBytes(encoded);
}

/**
 * Known EIP-712 domain fields for native USDC on each CCTPv2-supported EVM
 * chain. Populate as needed — verify on-chain via `DOMAIN_SEPARATOR()` before
 * relying on these in production.
 *
 * Arbitrum native USDC (`0xaf88d065e77c8cC2239327C5EDb3A432268e5831`) reports
 * `name = "USD Coin"`, `version = "2"`. Most Circle-issued native USDC uses
 * the same; bridged USDC.e variants differ.
 */
export const USDC_DOMAIN_FIELDS: Record<
  number,
  { name: string; version: string; address: string }
> = {
  42161: {
    name: "USD Coin",
    version: "2",
    address: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
  },
};

// ── helpers ──────────────────────────────────────────────────────────────────

function keccak256Hex(input: string): string {
  return `0x${bytesToHex(keccak_256(new TextEncoder().encode(input)))}`;
}

function keccak256HexBytes(hexInput: string): string {
  const bytes = new Uint8Array(hexInput.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(hexInput.slice(i * 2, i * 2 + 2), 16);
  }
  return `0x${bytesToHex(keccak_256(bytes))}`;
}

function strip(hex: string): string {
  return hex.replace(/^0x/, "").toLowerCase();
}

function uint256Hex(value: bigint): string {
  return value.toString(16).padStart(64, "0");
}

function addressHex(address: string): string {
  return address.replace(/^0x/, "").toLowerCase().padStart(64, "0");
}
