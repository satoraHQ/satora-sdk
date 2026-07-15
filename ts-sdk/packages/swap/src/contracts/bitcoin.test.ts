import { sha256 } from "@noble/hashes/sha2.js";
import { hex } from "@scure/base";
import { describe, expect, it } from "vitest";
import { bitcoinObservation, classifyBitcoinSpend } from "./bitcoin.js";

const preimage = new Uint8Array(32).fill(7);
const paymentHash = sha256(preimage);
const preimageHex = hex.encode(preimage);

// A plausible claim witness: <sig> <preimage> <1> <script>; refund omits it.
const claimWitness = ["3045deadbeef", preimageHex, "01", "aabbccdd"];
const refundWitness = ["3045deadbeef", "00", "aabbccdd"];

describe("classifyBitcoinSpend", () => {
  it("verifies a revealed preimage as a claim and returns it", () => {
    expect(classifyBitcoinSpend(claimWitness, paymentHash)).toEqual({
      spend: "claim",
      preimage,
    });
  });

  it("treats a witness without the preimage as a refund", () => {
    expect(classifyBitcoinSpend(refundWitness, paymentHash)).toEqual({
      spend: "refund",
    });
  });

  it("rejects a 32-byte element that doesn't match our payment hash", () => {
    const other = hex.encode(new Uint8Array(32).fill(9));
    expect(classifyBitcoinSpend(["3045", other], paymentHash)).toEqual({
      spend: "refund",
    });
  });
});

describe("bitcoinObservation", () => {
  const EXPECTED = 1000;

  it("maps funding states straight through when funded with the expected amount", () => {
    expect(
      bitcoinObservation(
        { funding: "absent", fundedSats: 0 },
        paymentHash,
        EXPECTED,
      ),
    ).toEqual({ observation: "absent" });
    expect(
      bitcoinObservation(
        { funding: "mempool", fundedSats: 1000 },
        paymentHash,
        EXPECTED,
      ),
    ).toEqual({ observation: "mempool" });
    expect(
      bitcoinObservation(
        { funding: "confirmed", fundedSats: 1000 },
        paymentHash,
        EXPECTED,
      ),
    ).toEqual({ observation: "confirmed" });
  });

  it("is invalid when a confirmed funding is below the expected amount", () => {
    expect(
      bitcoinObservation(
        { funding: "confirmed", fundedSats: 999 },
        paymentHash,
        EXPECTED,
      ),
    ).toEqual({ observation: "invalid" });
  });

  it("resolves a claim spend with the recovered preimage", () => {
    expect(
      bitcoinObservation(
        { funding: "confirmed", fundedSats: 0, spendWitness: claimWitness },
        paymentHash,
        EXPECTED,
      ),
    ).toEqual({ observation: "spent_claim", preimage });
  });

  it("resolves a refund spend", () => {
    expect(
      bitcoinObservation(
        { funding: "confirmed", fundedSats: 0, spendWitness: refundWitness },
        paymentHash,
        EXPECTED,
      ),
    ).toEqual({ observation: "spent_refund" });
  });
});
