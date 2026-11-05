import { describe, expect, it } from "vitest";
import { decodeSwapCreatedLog, htlcSwapKey } from "../src/evm/htlc.js";
import { REAL_LOG } from "./evm-funding-fixture.js";

// Vectors from the contracts' own `computeKey` (forge, HTLCErc20 v7 and
// HTLCNative v2), so a change to the TS encoding cannot go unnoticed.
const SENDER = "0x90f79bf6eb2c4f870365e785982e1f101e93b906";
const CLAIM = "0xfcb0b3c3c6320420b749bcd3e4400f0b9fdf682a";
const HASH = `0x${"11".repeat(32)}`;

describe("htlcSwapKey", () => {
  it("is HTLCErc20.computeKey", () => {
    expect(
      htlcSwapKey({
        preimageHash: HASH,
        amount: 123456789n,
        token: "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6",
        refundAddress: SENDER,
        claimAddress: CLAIM,
        timelock: 1_800_000_000,
      }),
    ).toBe(
      "0x2abd501afb31698cdfef281266c7bbbb553b7ebf62e0fc90a0bcc2b3f3e779bf",
    );
  });

  it("is HTLCNative.computeKey, with a zero asset word", () => {
    expect(
      htlcSwapKey({
        preimageHash: HASH,
        amount: 5n * 10n ** 18n,
        token: "0x0000000000000000000000000000000000000000",
        refundAddress: SENDER,
        claimAddress: CLAIM,
        timelock: 1_800_000_000,
      }),
    ).toBe(
      "0x4cd0e05b8b4d12bc31ab257c92f8424eb007f2550c4492f03eb21bfbd7f27f45",
    );
  });

  it("is the key a deployed HTLC emitted in SwapCreated", () => {
    const created = decodeSwapCreatedLog(REAL_LOG);
    if (!created) throw new Error("fixture log does not decode");
    expect(htlcSwapKey(created)).toBe(created.key);
  });
});
