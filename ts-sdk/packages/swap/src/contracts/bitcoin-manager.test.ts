import { sha256 } from "@noble/hashes/sha2.js";
import { hex } from "@scure/base";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HtlcObservation } from "../actions/types.js";
import type { BitcoinHtlcFacts } from "./bitcoin.js";
import {
  type BitcoinChainReader,
  BitcoinContractManager,
} from "./bitcoin-manager.js";
import type { HtlcRef } from "./types.js";

const preimage = new Uint8Array(32).fill(7);
const paymentHash = sha256(preimage);

const ref = {
  ledger: "bitcoin",
  address: "bcrt1qhtlc",
  preimageHash: hex.encode(paymentHash),
  expectedSats: 1000,
} satisfies HtlcRef;

const claimWitness = ["3045ab", hex.encode(preimage), "01", "aabb"];

class FakeReader implements BitcoinChainReader {
  facts: BitcoinHtlcFacts = { funding: "absent", fundedSats: 0 };
  getHtlcFacts = vi.fn(async () => this.facts);
}

describe("BitcoinContractManager", () => {
  let reader: FakeReader;

  beforeEach(() => {
    reader = new FakeReader();
  });

  const build = () => BitcoinContractManager.fromDeps({ reader });

  it("rejects non-bitcoin HTLCs", async () => {
    await expect(
      build().register({ ledger: "lightning", paymentHash: "ab" }),
    ).rejects.toThrow(/can't track/);
  });

  it("maps funding facts to observations", async () => {
    const m = build();
    await m.register(ref);
    expect(reader.getHtlcFacts).toHaveBeenCalledWith(ref.address);
    expect(m.getState(ref)).toBe("absent");

    reader.facts = { funding: "confirmed", fundedSats: 1000 };
    await m.refresh();
    expect(m.getState(ref)).toBe("confirmed");
  });

  it("is invalid when funded below the expected amount", async () => {
    const m = build();
    reader.facts = { funding: "confirmed", fundedSats: 999 };
    await m.register(ref);
    expect(m.getState(ref)).toBe("invalid");
  });

  it("classifies a claim spend and recovers the verified preimage", async () => {
    const m = build();
    reader.facts = {
      funding: "confirmed",
      fundedSats: 0,
      spendWitness: claimWitness,
    };
    await m.register(ref);
    expect(m.getState(ref)).toBe("spent_claim");
    expect(m.getPreimage(ref)).toEqual(preimage);
  });

  it("classifies a timelock refund spend", async () => {
    const m = build();
    reader.facts = {
      funding: "confirmed",
      fundedSats: 0,
      spendWitness: ["3045ab", "00"],
    };
    await m.register(ref);
    expect(m.getState(ref)).toBe("spent_refund");
    expect(m.getPreimage(ref)).toBeUndefined();
  });

  it("notifies listeners on change and never downgrades a spend", async () => {
    const m = build();
    const seen: HtlcObservation[] = [];
    m.onEvent((_r, s) => seen.push(s));
    reader.facts = {
      funding: "confirmed",
      fundedSats: 0,
      spendWitness: claimWitness,
    };
    await m.register(ref);
    expect(m.getState(ref)).toBe("spent_claim");
    // A later poll that only sees funding must not revert the spend.
    reader.facts = { funding: "confirmed", fundedSats: 1000 };
    await m.refresh();
    expect(m.getState(ref)).toBe("spent_claim");
    expect(seen).toEqual(["spent_claim"]);
  });

  it("reports chainNow only once a chain time is provided", async () => {
    const withClock = BitcoinContractManager.fromDeps({
      reader,
      chainTime: async () => 1_700_000_000_000,
    });
    expect(withClock.chainNow(ref)).toBeUndefined();
    await withClock.refresh();
    expect(withClock.chainNow(ref)).toBe(1_700_000_000_000);
  });
});
