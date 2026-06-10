import { Transaction } from "@arkade-os/sdk";
import { base64 } from "@scure/base";

export interface SignedEscrowTx {
  signedPsbt: string;
  txid: string;
}

/**
 * Sign input 0 of an Arkade Arkade transaction PSBT with `secretKey`.
 *
 * Equivalent to `signEscrowArkTx` from `@lendasat/lendaswap-sdk-pure`,
 * lifted into this SDK so consumers don't have to pull in Lendaswap's
 * EVM dependency stack (zerodev/viem) just to sign one tapscript leaf.
 *
 * The PSBT must already carry the cooperative tapleaf script, control
 * block, and the sighash type on input 0. The arbiter (server) attaches
 * those when it builds the release tx. Does not finalize.
 */
export function signEscrowArkTx(
  psbtB64: string,
  secretKey: Uint8Array,
): SignedEscrowTx {
  const tx = Transaction.fromPSBT(base64.decode(psbtB64));
  tx.signIdx(secretKey, 0);
  return {
    signedPsbt: base64.encode(tx.toPSBT()),
    txid: tx.id,
  };
}

/** Same as {@link signEscrowArkTx} but for an array of checkpoint PSBTs. */
export function signEscrowCheckpoints(
  psbtB64s: string[],
  secretKey: Uint8Array,
): string[] {
  return psbtB64s.map((b) => signEscrowArkTx(b, secretKey).signedPsbt);
}
