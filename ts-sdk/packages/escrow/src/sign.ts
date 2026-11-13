import { Transaction } from "@arkade-os/sdk";
import { base64 } from "@scure/base";

export interface SignedEscrowTx {
  signedPsbt: string;
  txid: string;
}

/** Whether `script` is a P2TR output script (`OP_1 <32-byte key>`). */
function isP2tr(script: Uint8Array | undefined): boolean {
  return script?.length === 34 && script[0] === 0x51 && script[1] === 0x20;
}

/**
 * Parse an escrow PSBT built by the arbiter.
 *
 * ark-core records the spend script as `witnessScript` on taproot inputs, next
 * to the tapleaf script that actually matters. A strict PSBT parser rejects
 * that ("witnessScript without P2WSH"), so the field is dropped from taproot
 * inputs and the result is parsed again with every check on, the taproot
 * commitment check included. Nothing the signer or the arbiter needs is lost:
 * the leaf script, control block and prevout stay, and the arbiter only takes
 * the signatures from what comes back.
 */
function parseEscrowPsbt(psbt: Uint8Array): Transaction {
  const lenient = Transaction.fromPSBT(psbt, { disableScriptCheck: true });
  let stripped = false;
  for (let i = 0; i < lenient.inputsLength; i++) {
    const input = lenient.getInput(i);
    if (input.witnessScript && isP2tr(input.witnessUtxo?.script)) {
      lenient.updateInput(i, { witnessScript: undefined }, true);
      stripped = true;
    }
  }
  return Transaction.fromPSBT(stripped ? lenient.toPSBT() : psbt);
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
  const tx = parseEscrowPsbt(base64.decode(psbtB64));
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
