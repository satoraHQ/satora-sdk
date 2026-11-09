import { Transaction } from "@arkade-os/sdk";
import { hex } from "@scure/base";
import { describe, expect, it } from "vitest";
import { ensureConditionWitness } from "../src/redeem/arkade-condition.js";

const preimage = new Uint8Array(32).fill(7);
const claimScriptByte = new Uint8Array(32).fill(9);

/** P2TR scriptPubKey: OP_1 || 32-byte x-only key */
function p2trScript(): Uint8Array {
  return hex.decode(`5120${"ab".repeat(32)}`);
}

function taprootCheckpoint(): Transaction {
  const tx = new Transaction({ allowUnknownInputs: true, version: 3 });
  tx.addInput({
    txid: new Uint8Array(32).fill(1),
    index: 0,
    witnessUtxo: { script: p2trScript(), amount: 5_000n },
  });
  return tx;
}

describe("continueArkadeClaim checkpoint finalize prep", () => {
  it("rejects witnessScript on a taproot (non-P2WSH) checkpoint input", () => {
    const checkpoint = taprootCheckpoint();
    expect(() =>
      checkpoint.updateInput(0, { witnessScript: claimScriptByte }),
    ).toThrow(/witnessScript without P2WSH/);
  });

  it("allows finalize prep that only ensures ConditionWitness", () => {
    const checkpoint = taprootCheckpoint();
    expect(() => ensureConditionWitness(checkpoint, 0, preimage)).not.toThrow();
    // Round-trip like a sign/finalize path would.
    expect(() => Transaction.fromPSBT(checkpoint.toPSBT())).not.toThrow();
  });
});
