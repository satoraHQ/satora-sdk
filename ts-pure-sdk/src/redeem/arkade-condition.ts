import {
  ConditionWitness,
  getArkPsbtFields,
  setArkPsbtField,
  type Transaction,
} from "@arkade-os/sdk";

/**
 * Set the condition witness (the HTLC preimage) on an input unless the input
 * already carries one.
 *
 * `setArkPsbtField` appends, and arkd (btcd PSBT decoder) rejects a PSBT with
 * a duplicate proprietary key. The server may echo the witness we attached
 * before submit on the checkpoints it returns, so every later insertion must
 * be idempotent.
 */
export function ensureConditionWitness(
  tx: Transaction,
  inputIndex: number,
  preimage: Uint8Array,
): void {
  if (getArkPsbtFields(tx, inputIndex, ConditionWitness).length > 0) return;
  setArkPsbtField(tx, inputIndex, ConditionWitness, [preimage]);
}

/**
 * Attach the preimage to every ark tx input and to every checkpoint input
 * before submit. arkd requires each checkpoint to carry the same condition
 * witness as the ark tx input that spends it.
 */
export function attachClaimConditionWitness(
  arkTx: Transaction,
  checkpoints: Transaction[],
  preimage: Uint8Array,
): void {
  for (let i = 0; i < arkTx.inputsLength; i++) {
    ensureConditionWitness(arkTx, i, preimage);
  }
  for (const checkpoint of checkpoints) {
    for (let i = 0; i < checkpoint.inputsLength; i++) {
      ensureConditionWitness(checkpoint, i, preimage);
    }
  }
}
