/**
 * ZeroDev Kernel V3.3 ERC-1271 support for the SDK-controlled EVM key.
 *
 * The sponsored-UserOp claim path delegates the SDK's deterministic EVM
 * address to Kernel V3.3 via EIP-7702. From then on the address has code,
 * so Permit2 verifies signatures through `isValidSignature` on the Kernel
 * account instead of `ecrecover`. Kernel accepts only its own envelope:
 *
 *   `0x00` (sudo-mode selector → 7702 root validator, i.e. the EOA's key)
 *   `||` ECDSA over the EIP-712 digest of `Kernel(bytes32 hash)` with domain
 *   `name="Kernel"`, `version="0.3.3"`, `chainId`, `verifyingContract` = the
 *   account itself.
 *
 * Mirrors `rust-sdk/src/aa/kernel.rs` (`erc1271_wrapped_digest`,
 * `wrap_erc1271_signature`). Sources: Kernel v3.3
 * `src/core/ValidationManager.sol::_toWrappedHash`,
 * `src/utils/ValidationTypeLib.sol::decodeSignature`.
 */

import { keccak_256 } from "@noble/hashes/sha3.js";
import {
  hexToBytes as nobleFromHex,
  bytesToHex as nobleToHex,
} from "@noble/hashes/utils.js";

/**
 * Kernel V3.3 implementation — the EIP-7702 delegation target. Same
 * address on every chain. Must match the backend's
 * `KERNEL_DELEGATION_TARGET_V3_3`.
 */
export const KERNEL_DELEGATION_TARGET =
  "0xd6CEDDe84be40893d153Be9d467CD6aD37875b28";

/** EIP-712 domain `version` Kernel V3.3 uses in `isValidSignature`. */
export const KERNEL_VERSION = "0.3.3";

/** EIP-7702 delegation designator prefix (`0xef0100 || address`). */
const EIP7702_PREFIX = "ef0100";

/** Validator-selection prefix for the 7702 root-validator ("sudo") path. */
const ERC1271_SUDO_MODE_PREFIX = "00";

const EIP712_DOMAIN_TYPEHASH = keccak(
  utf8(
    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
  ),
);
const KERNEL_WRAPPER_TYPEHASH = keccak(utf8("Kernel(bytes32 hash)"));

/**
 * Return the EIP-7702 delegation target encoded in `code`, or `null` if
 * the code is empty or not a delegation designator.
 */
export function parseEip7702Delegation(
  code: string | null | undefined,
): string | null {
  const hex = (code ?? "").replace(/^0x/i, "").toLowerCase();
  if (hex.length !== 46 || !hex.startsWith(EIP7702_PREFIX)) {
    return null;
  }
  return `0x${hex.slice(6)}`;
}

/** True if `code` is an EIP-7702 delegation to Kernel V3.3. */
export function isKernelDelegation(code: string | null | undefined): boolean {
  const target = parseEip7702Delegation(code);
  return target !== null && target === KERNEL_DELEGATION_TARGET.toLowerCase();
}

/**
 * EIP-712 digest a Kernel-delegated EOA must sign so that
 * `isValidSignature(innerHash, …)` on the account passes.
 *
 * The result is a raw EIP-712 hash: sign it with plain ECDSA (no EIP-191
 * prefix), exactly like the inner Permit2 digest.
 */
export function kernelErc1271Digest(
  innerHash: string,
  account: string,
  chainId: number,
): string {
  const domainSeparator = keccak(
    concat(
      EIP712_DOMAIN_TYPEHASH,
      keccak(utf8("Kernel")),
      keccak(utf8(KERNEL_VERSION)),
      uint256(BigInt(chainId)),
      address(account),
    ),
  );
  const structHash = keccak(
    concat(KERNEL_WRAPPER_TYPEHASH, bytes32(innerHash)),
  );
  return `0x${nobleToHex(
    keccak_256(concat(nobleFromHex("1901"), domainSeparator, structHash)),
  )}`;
}

/**
 * Wrap a 65-byte `r || s || v` signature for Kernel's `isValidSignature`:
 * prepends the `0x00` sudo-mode selector, routing verification to the
 * delegated EOA's own key.
 */
export function wrapKernelErc1271Signature(signature: string): string {
  const hex = signature.replace(/^0x/i, "");
  if (hex.length !== 130) {
    throw new Error(
      `wrapKernelErc1271Signature: expected a 65-byte signature, got ${hex.length / 2} bytes`,
    );
  }
  return `0x${ERC1271_SUDO_MODE_PREFIX}${hex}`;
}

function keccak(data: Uint8Array): Uint8Array {
  return keccak_256(data);
}

function utf8(s: string): Uint8Array {
  return new TextEncoder().encode(s);
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}

function uint256(value: bigint): Uint8Array {
  return nobleFromHex(value.toString(16).padStart(64, "0"));
}

function address(value: string): Uint8Array {
  const hex = value.replace(/^0x/i, "").toLowerCase();
  if (hex.length !== 40) {
    throw new Error(`kernelErc1271Digest: invalid address ${value}`);
  }
  return nobleFromHex(hex.padStart(64, "0"));
}

function bytes32(value: string): Uint8Array {
  const hex = value.replace(/^0x/i, "").toLowerCase();
  if (hex.length !== 64) {
    throw new Error(
      `kernelErc1271Digest: expected a 32-byte hash, got ${value}`,
    );
  }
  return nobleFromHex(hex);
}
