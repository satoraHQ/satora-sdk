/**
 * Verify a revealed witness element is the preimage for a swap's hash lock.
 *
 * The hash length selects the algorithm, so callers don't need to know the swap
 * direction: a 20-byte lock is HASH160 = `ripemd160(sha256(preimage))`
 * (`btc_to_arkade`, matching Bitcoin's `OP_HASH160` and the Arkade VHTLC); any
 * other length is `sha256(preimage)` (32 bytes, the EVM/Lightning directions,
 * i.e. the Lightning payment_hash).
 */
import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { hex } from "@scure/base";

export function preimageMatches(
  element: Uint8Array,
  hashLock: Uint8Array,
): boolean {
  const digest =
    hashLock.length === 20 ? ripemd160(sha256(element)) : sha256(element);
  return hex.encode(digest) === hex.encode(hashLock);
}
