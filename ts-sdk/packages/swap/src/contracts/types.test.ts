import { describe, expect, it } from "vitest";
import { type HtlcRef, htlcKey } from "./types.js";

describe("htlcKey", () => {
  const cases: Array<[HtlcRef, string]> = [
    [
      {
        ledger: "arkade",
        script: "51ab",
        address: "ark1q",
        preimageHash: "h",
        expectedSats: 0,
        params: {},
      },
      "arkade:51ab",
    ],
    [
      {
        ledger: "bitcoin",
        address: "bc1q",
        preimageHash: "h",
        expectedSats: 0,
      },
      "bitcoin:bc1q",
    ],
    [
      {
        ledger: "evm",
        chainId: 137,
        htlc: "0xabc",
        preimageHash: "0xdef",
        claimAddress: "0xc1",
        expectedAmount: 0n,
      },
      "evm:137:0xdef",
    ],
    [{ ledger: "lightning", paymentHash: "ph" }, "lightning:ph"],
  ];

  it.each(cases)("%o → %s", (ref, key) => {
    expect(htlcKey(ref)).toBe(key);
  });

  it("distinguishes the same EVM HTLC on different chains", () => {
    const base = {
      ledger: "evm",
      htlc: "0xabc",
      preimageHash: "0xdef",
      claimAddress: "0xc1",
      expectedAmount: 0n,
    } as const;
    expect(htlcKey({ ...base, chainId: 1 })).not.toBe(
      htlcKey({ ...base, chainId: 137 }),
    );
  });
});
