/**
 * Permit2 signing for `executeAndCreateWithPermit2` using the SDK's
 * HD-derived EVM key.
 *
 * Wraps the existing `buildPermit2FundingDigest` + `signEvmDigest` and
 * returns the compact 65-byte signature format (`r ‖ s ‖ v`) that
 * `HTLCCoordinator.executeAndCreateWithPermit2` expects, ready to POST
 * to the backend's gasless-funding endpoint.
 */

import {
  buildPermit2FundingDigest,
  type Permit2FundingParams,
} from "../evm/coordinator.js";
import { signEvmDigest } from "../evm/signing.js";

/** Parameters for signing a Permit2 witness with the SDK's HD key. */
export interface SignPermit2Params {
  /** Signer's secp256k1 private key. */
  secretKey: Uint8Array | string;
  /** All fields of the Permit2 witness + TokenPermissions. */
  funding: Permit2FundingParams;
}

/** Signed Permit2 witness in the shape the backend's gasless endpoint expects. */
export interface SignedPermit2Witness {
  /** Random Permit2 nonce (stringified for API transport). */
  nonce: string;
  /** Signature deadline (unix seconds). */
  deadline: number;
  /** Compact signature: `0x` + r(32) + s(32) + v(1), 65 bytes. */
  signature: string;
}

/**
 * Signs the Permit2 EIP-712 digest using the SDK's derived key and packs
 * the signature in the compact `r ‖ s ‖ v` form the backend submitter
 * forwards to Permit2 on-chain.
 */
export function signPermit2Witness(
  params: SignPermit2Params,
): SignedPermit2Witness {
  const digest = buildPermit2FundingDigest(params.funding);
  const sig = signEvmDigest(params.secretKey, digest);

  const r = sig.r.replace(/^0x/, "");
  const s = sig.s.replace(/^0x/, "");
  const v = sig.v.toString(16).padStart(2, "0");
  if (r.length !== 64 || s.length !== 64) {
    throw new Error(
      `Malformed signature scalars: r=${r.length}, s=${s.length}`,
    );
  }

  return {
    nonce: params.funding.nonce.toString(),
    deadline: Number(params.funding.deadline),
    signature: `0x${r}${s}${v}`,
  };
}
