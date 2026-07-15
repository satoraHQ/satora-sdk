import { ConditionWitness, setArkPsbtField, Transaction } from "@arkade-os/sdk";
import { sha256 } from "@noble/hashes/sha2.js";
import { base64 } from "@scure/base";
import { describe, expect, it } from "vitest";
import type { HtlcObservation } from "../actions/types.js";
import {
  type ArkadeVtxoFacts,
  arkadeObservation,
  classifyArkadeSpend,
  fetchArkadeSpend,
} from "./arkade.js";

describe("arkadeObservation", () => {
  const cases: Array<[ArkadeVtxoFacts, HtlcObservation]> = [
    [{ funded: false }, "absent"],
    [{ funded: true }, "confirmed"], // preconfirmed or settled
    [{ funded: true, sufficient: true }, "confirmed"],
    [{ funded: true, sufficient: false }, "invalid"], // funded but short
    [{ funded: false, sufficient: false }, "absent"], // not funded wins
    [{ funded: true, spend: "claim" }, "spent_claim"],
    [{ funded: true, spend: "refund" }, "spent_refund"],
    // a resolved spend dominates the funded flag
    [{ funded: false, spend: "claim" }, "spent_claim"],
  ];

  it.each(cases)("%o → %s", (facts, expected) => {
    expect(arkadeObservation(facts)).toBe(expected);
  });
});

/** Build a base64 PSBT with one input, optionally carrying a ConditionWitness. */
function spendPsbt(preimage?: Uint8Array): string {
  const tx = new Transaction({ allowUnknownInputs: true });
  tx.addInput({ txid: new Uint8Array(32).fill(1), index: 0 });
  if (preimage) setArkPsbtField(tx, 0, ConditionWitness, [preimage]);
  return base64.encode(tx.toPSBT());
}

describe("classifyArkadeSpend", () => {
  const preimage = new Uint8Array(32).fill(7);
  const paymentHash = sha256(preimage);

  it("verifies a revealed preimage as a claim and returns it", () => {
    expect(classifyArkadeSpend(spendPsbt(preimage), paymentHash)).toEqual({
      spend: "claim",
      preimage,
    });
  });

  it("treats a spend without the condition field as a refund", () => {
    expect(classifyArkadeSpend(spendPsbt(), paymentHash)).toEqual({
      spend: "refund",
    });
  });

  it("rejects a condition field that doesn't match our payment hash", () => {
    // A present-but-wrong preimage must not be read as our claim — proves we
    // verify the hash rather than merely detecting the field's presence.
    const other = new Uint8Array(32).fill(9);
    expect(classifyArkadeSpend(spendPsbt(other), paymentHash)).toEqual({
      spend: "refund",
    });
  });
});

describe("fetchArkadeSpend", () => {
  const preimage = new Uint8Array(32).fill(7);
  const paymentHash = sha256(preimage);

  it("classifies the spending tx returned by the indexer", async () => {
    const source = {
      getVirtualTxs: async () => ({ txs: [spendPsbt(preimage)] }),
    };
    expect(await fetchArkadeSpend(source, "abc", paymentHash)).toEqual({
      spend: "claim",
      preimage,
    });
  });

  it("returns undefined when the indexer has no spending tx yet", async () => {
    const source = { getVirtualTxs: async () => ({ txs: [] }) };
    expect(await fetchArkadeSpend(source, "abc", paymentHash)).toBeUndefined();
  });
});
