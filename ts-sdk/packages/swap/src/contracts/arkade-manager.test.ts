import {
  ConditionWitness,
  setArkPsbtField,
  Transaction,
  type VirtualCoin,
} from "@arkade-os/sdk";
import { sha256 } from "@noble/hashes/sha2.js";
import { base64, hex } from "@scure/base";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HtlcObservation } from "../actions/types.js";
import { ArkadeContractManager, type ArkadeIndexer } from "./arkade-manager.js";
import type { HtlcRef } from "./types.js";

const preimage = new Uint8Array(32).fill(7);
const paymentHash = sha256(preimage);

const ref = {
  ledger: "arkade",
  script: "deadbeef",
  address: "ark1qexample",
  preimageHash: hex.encode(paymentHash),
  expectedSats: 1000,
  params: { sender: "ab12", receiver: "cd34" },
} satisfies HtlcRef;

/** A base64 PSBT with one input, optionally carrying a ConditionWitness. */
function spendPsbt(secret?: Uint8Array): string {
  const tx = new Transaction({ allowUnknownInputs: true });
  tx.addInput({ txid: new Uint8Array(32).fill(1), index: 0 });
  if (secret) setArkPsbtField(tx, 0, ConditionWitness, [secret]);
  return base64.encode(tx.toPSBT());
}

/** Build a minimal vtxo; only the fields the observer reads matter. */
function vtxo(p: {
  state: "preconfirmed" | "settled" | "swept" | "spent";
  spentBy?: string;
  value?: number;
}): VirtualCoin {
  return {
    virtualStatus: { state: p.state },
    isSpent: p.state === "spent",
    spentBy: p.spentBy,
    value: p.value ?? 1000,
  } as unknown as VirtualCoin;
}

class FakeIndexer implements ArkadeIndexer {
  vtxos: VirtualCoin[] = [];
  txs: string[] = [];
  getVtxos = vi.fn(async () => ({ vtxos: this.vtxos }));
  getVirtualTxs = vi.fn(async () => ({ txs: this.txs }));
}

describe("ArkadeContractManager", () => {
  let indexer: FakeIndexer;

  beforeEach(() => {
    indexer = new FakeIndexer();
  });

  const build = () => ArkadeContractManager.fromDeps({ indexer });

  it("rejects non-arkade HTLCs", async () => {
    await expect(
      build().register({ ledger: "lightning", paymentHash: "ab" }),
    ).rejects.toThrow(/can't track/);
  });

  it("is absent when the VHTLC has no vtxos", async () => {
    const m = build();
    await m.register(ref);
    expect(indexer.getVtxos).toHaveBeenCalledWith({ scripts: [ref.script] });
    expect(m.getState(ref)).toBe("absent");
  });

  it("is confirmed once funded with the expected amount", async () => {
    const m = build();
    indexer.vtxos = [vtxo({ state: "settled", value: 1000 })];
    await m.register(ref);
    expect(m.getState(ref)).toBe("confirmed");
  });

  it("is invalid when funded below the expected amount", async () => {
    const m = build();
    indexer.vtxos = [vtxo({ state: "settled", value: 999 })];
    await m.register(ref);
    expect(m.getState(ref)).toBe("invalid");
  });

  it("classifies a claim spend and recovers the verified preimage", async () => {
    const m = build();
    indexer.vtxos = [vtxo({ state: "spent", spentBy: "spendtx" })];
    indexer.txs = [spendPsbt(preimage)];
    await m.register(ref);
    expect(indexer.getVirtualTxs).toHaveBeenCalledWith(["spendtx"]);
    expect(m.getState(ref)).toBe("spent_claim");
    expect(m.getPreimage(ref)).toEqual(preimage);
  });

  it("classifies a timelock refund spend (no preimage revealed)", async () => {
    const m = build();
    indexer.vtxos = [vtxo({ state: "spent", spentBy: "spendtx" })];
    indexer.txs = [spendPsbt()];
    await m.register(ref);
    expect(m.getState(ref)).toBe("spent_refund");
    expect(m.getPreimage(ref)).toBeUndefined();
  });

  it("notifies listeners and re-observes on refresh", async () => {
    const m = build();
    const seen: HtlcObservation[] = [];
    m.onEvent((_r, s) => seen.push(s));
    await m.register(ref); // absent

    indexer.vtxos = [vtxo({ state: "settled" })];
    await m.refresh();
    expect(m.getState(ref)).toBe("confirmed");
    expect(seen).toEqual(["absent", "confirmed"]);
  });

  it("never downgrades a resolved spend back to a funding state", async () => {
    const m = build();
    indexer.vtxos = [vtxo({ state: "spent", spentBy: "spendtx" })];
    indexer.txs = [spendPsbt(preimage)];
    await m.register(ref);
    expect(m.getState(ref)).toBe("spent_claim");
    // A later poll that only sees a funded vtxo must not revert the spend.
    indexer.vtxos = [vtxo({ state: "settled" })];
    await m.refresh();
    expect(m.getState(ref)).toBe("spent_claim");
  });

  it("reports chainNow only once a chain time is provided", async () => {
    const withClock = ArkadeContractManager.fromDeps({
      indexer,
      chainTime: async () => 1_700_000_000_000,
    });
    expect(withClock.chainNow(ref)).toBeUndefined();
    await withClock.refresh();
    expect(withClock.chainNow(ref)).toBe(1_700_000_000_000);
  });

  it("unregisters and forgets state", async () => {
    const m = build();
    indexer.vtxos = [vtxo({ state: "settled" })];
    await m.register(ref);
    expect(m.getState(ref)).toBe("confirmed");
    await m.unregister(ref);
    expect(m.getState(ref)).toBeUndefined();
  });
});
