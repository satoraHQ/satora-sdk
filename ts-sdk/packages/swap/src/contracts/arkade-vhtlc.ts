/**
 * Build the Arkade {@link HtlcRef} for a swap's VHTLC leg from the plain swap
 * fields, deriving the pkScript and serialized contract params the
 * `ArkadeContractManager` needs to register it for watching.
 *
 * This is the one place that reconstructs the VHTLC from a swap, mirroring the
 * legacy SDK's `VHTLC.Script` construction, so the mapper and manager stay free
 * of VHTLC internals.
 */
import { VHTLC, VHTLCContractHandler } from "@arkade-os/sdk";
import { ripemd160 } from "@noble/hashes/legacy.js";
import { hex } from "@scure/base";
import type { HtlcRef } from "./types.js";

type ArkadeRef = Extract<HtlcRef, { ledger: "arkade" }>;

export type ArkadeVhtlcInput = {
  /** Pubkey hex (x-only or compressed) for each VHTLC role. */
  senderPk: string;
  receiverPk: string;
  serverPk: string;
  /**
   * The swap's `hash_lock`, hex, `0x`-prefixed or not. Either `sha256(preimage)`
   * (32 bytes — most directions) or the `ripemd160(sha256(preimage))` HASH160 the
   * VHTLC commits to directly (20 bytes — `btc_to_arkade`). Length disambiguates.
   */
  hashLock: string;
  /** The VHTLC address the server reported. */
  address: string;
  /** Absolute CLTV refund locktime (unix seconds). */
  refundLocktime: number;
  /** Relative unilateral delays (seconds). */
  unilateralClaimDelay: number;
  unilateralRefundDelay: number;
  unilateralRefundWithoutReceiverDelay: number;
  /** Expected funding amount in sats (a short funding is `invalid`, not confirmed). */
  expectedSats: number;
};

function strip0x(value: string): string {
  return value.startsWith("0x") ? value.slice(2) : value;
}

/** Normalize a pubkey hex to the 32-byte x-only form the VHTLC expects. */
function xOnly(pubKeyHex: string): Uint8Array {
  const bytes = hex.decode(strip0x(pubKeyHex));
  if (bytes.length === 33) return bytes.slice(1);
  if (bytes.length === 32) return bytes;
  throw new Error(`invalid public key length: ${bytes.length}`);
}

const seconds = (value: number) =>
  ({ type: "seconds", value: BigInt(value) }) as const;

/**
 * The VHTLC always commits to `ripemd160(sha256(preimage))` (HASH160). When
 * `hashLock` is `sha256(preimage)` (32 bytes) we hash it once more to get that;
 * when it is already the HASH160 (20 bytes, `btc_to_arkade`) we use it as-is. The
 * ref keeps `preimageHash` as the raw `hashLock` — whatever length — and the spend
 * classifier verifies a revealed preimage against it, auto-detecting the algorithm
 * from that length.
 */
export function buildArkadeVhtlcRef(input: ArkadeVhtlcInput): ArkadeRef {
  const hashLockBytes = hex.decode(strip0x(input.hashLock));
  const params = {
    sender: xOnly(input.senderPk),
    receiver: xOnly(input.receiverPk),
    server: xOnly(input.serverPk),
    preimageHash:
      hashLockBytes.length === 20 ? hashLockBytes : ripemd160(hashLockBytes),
    refundLocktime: BigInt(input.refundLocktime),
    unilateralClaimDelay: seconds(input.unilateralClaimDelay),
    unilateralRefundDelay: seconds(input.unilateralRefundDelay),
    unilateralRefundWithoutReceiverDelay: seconds(
      input.unilateralRefundWithoutReceiverDelay,
    ),
  };
  const vhtlc = new VHTLC.Script(params);
  return {
    ledger: "arkade",
    script: hex.encode(vhtlc.pkScript),
    address: input.address,
    preimageHash: strip0x(input.hashLock),
    expectedSats: input.expectedSats,
    params: VHTLCContractHandler.serializeParams(params),
  };
}
