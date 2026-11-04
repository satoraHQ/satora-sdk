import {
  ConditionWitness,
  getArkPsbtFields,
  setArkPsbtField,
  Transaction,
} from "@arkade-os/sdk";
import { base64, hex } from "@scure/base";
import { describe, expect, it } from "vitest";
import {
  attachClaimConditionWitness,
  ensureConditionWitness,
} from "../src/redeem/arkade-condition.js";

const preimage = new Uint8Array(32).fill(7);

function txWithInputs(n: number): Transaction {
  const tx = new Transaction({ allowUnknownInputs: true, version: 3 });
  for (let i = 0; i < n; i++) {
    tx.addInput({ txid: new Uint8Array(32).fill(i + 1), index: i });
  }
  return tx;
}

function conditions(tx: Transaction, input: number): string[][] {
  return getArkPsbtFields(tx, input, ConditionWitness).map((w) =>
    w.map(hex.encode),
  );
}

describe("attachClaimConditionWitness", () => {
  it("puts the preimage on every ark tx input and every checkpoint input", () => {
    const arkTx = txWithInputs(2);
    const checkpoints = [txWithInputs(1), txWithInputs(1)];

    attachClaimConditionWitness(arkTx, checkpoints, preimage);

    for (let i = 0; i < 2; i++) {
      expect(conditions(arkTx, i)).toEqual([[hex.encode(preimage)]]);
      expect(conditions(checkpoints[i], 0)).toEqual([[hex.encode(preimage)]]);
    }
  });
});

describe("ensureConditionWitness", () => {
  it("does not duplicate a witness the server echoed back", () => {
    const checkpoint = txWithInputs(1);
    setArkPsbtField(checkpoint, 0, ConditionWitness, [preimage]);
    // Round-trip through PSBT bytes like a server response.
    const echoed = Transaction.fromPSBT(
      base64.decode(base64.encode(checkpoint.toPSBT())),
    );

    ensureConditionWitness(echoed, 0, preimage);

    expect(conditions(echoed, 0)).toEqual([[hex.encode(preimage)]]);
    // Serialising must not produce a duplicate proprietary key.
    expect(() => Transaction.fromPSBT(echoed.toPSBT())).not.toThrow();
  });

  it("adds the witness when the server stripped it", () => {
    const checkpoint = txWithInputs(1);
    expect(conditions(checkpoint, 0)).toEqual([]);

    ensureConditionWitness(checkpoint, 0, preimage);

    expect(conditions(checkpoint, 0)).toEqual([[hex.encode(preimage)]]);
  });
});
