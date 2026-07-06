import { describe, expect, it } from "vitest";
import { netTargetSourcePinned } from "../src/compose-quote.js";

describe("netTargetSourcePinned (BTC→EVM source-pinned)", () => {
  it("subtracts the full flat bridge fee, not scaled by the BTC-side fee ratio", () => {
    // 1% BTC-side fee: btc=1_000_000, effective=990_000 → r=0.99.
    // Gross DEX output G=1_000_000; flat bridge fee B=10_000; delivered=G−B=990_000.
    const net = netTargetSourcePinned(1_000_000n, 990_000n, 990_000n, 10_000n);

    // Correct: (G·r) − B = 990_000 − 10_000 = 980_000.
    expect(net).toBe(980_000n);
    // The old formula scaled the *net*: delivered·r = 990_000·0.99 = 980_100,
    // overstating by B·(1−r) = 100. Guard against regressing to that.
    expect(net).not.toBe(980_100n);
  });

  it("is unchanged when there is no bridge fee", () => {
    // bridgeFee = 0 → plain net·r (matches the pre-fix non-bridge behavior).
    const net = netTargetSourcePinned(1_000_000n, 990_000n, 500_000n, 0n);
    expect(net).toBe((990_000n * 500_000n) / 1_000_000n); // 495_000
  });

  it("clamps to zero when fees exceed the amount", () => {
    expect(netTargetSourcePinned(0n, 0n, 100n, 5n)).toBe(0n); // no source
    expect(netTargetSourcePinned(1_000n, -1n, 100n, 5n)).toBe(0n); // fees > source
    // scaled gross below the bridge fee → 0, never negative.
    expect(netTargetSourcePinned(1_000n, 1n, 100n, 50n)).toBe(0n);
  });
});
