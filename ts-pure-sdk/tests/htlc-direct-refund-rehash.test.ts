import { describe, expect, it } from "vitest";
import {
  encodeHtlcErc20RefundBySigCallData,
  encodeHtlcErc20RehashCallData,
  encodeHtlcNativeRefundBySigCallData,
  encodeHtlcNativeRehashCallData,
  refundForAsOwnRefund,
} from "../src/evm/index.js";

/**
 * The lock terms of `contracts/test/HTLC*Domain.t.sol::test_refundDigestVector`
 * and `test_rehashDigestVector`; the expected calldata is `cast calldata`
 * over the v7 / native v2 ABI.
 */
const terms = {
  preimageHash:
    "0x1111111111111111111111111111111111111111111111111111111111111111",
  amount: 100_000_000n,
  token: "0x2222222222222222222222222222222222222222",
  timelock: 1_800_000_000,
};
const next = {
  newPreimageHash:
    "0x4444444444444444444444444444444444444444444444444444444444444444",
  newTimelock: 1_800_007_200,
};
const r = "0x5555555555555555555555555555555555555555555555555555555555555555";
const s = "0x6666666666666666666666666666666666666666666666666666666666666666";
const htlc = "0x1000000000000000000000000000000000000001";

const words = (...w: string[]) => w.map((x) => x.replace(/^0x/, "")).join("");
const hash = terms.preimageHash;
const amount =
  "0000000000000000000000000000000000000000000000000000000005f5e100";
const token = terms.token.slice(2).padStart(64, "0");
const timelock =
  "000000000000000000000000000000000000000000000000000000006b49d200";
const newTimelock =
  "000000000000000000000000000000000000000000000000000000006b49ee20";
const v27 = "000000000000000000000000000000000000000000000000000000000000001b";
const v28 = "000000000000000000000000000000000000000000000000000000000000001c";

describe("refundBySig calldata", () => {
  it("matches the HTLCErc20 v7 ABI", () => {
    const call = encodeHtlcErc20RefundBySigCallData(htlc, {
      ...terms,
      v: 27,
      r,
      s,
    });
    expect(call.to).toBe(htlc);
    expect(call.functionSignature).toBe(
      "refundBySig(bytes32,uint256,address,uint256,uint8,bytes32,bytes32)",
    );
    expect(call.data).toBe(
      `0x072c420d${words(hash, amount, token, timelock, v27, r, s)}`,
    );
  });

  it("matches the HTLCNative v2 ABI", () => {
    const call = encodeHtlcNativeRefundBySigCallData(htlc, {
      preimageHash: terms.preimageHash,
      amount: terms.amount,
      timelock: terms.timelock,
      v: 27,
      r,
      s,
    });
    expect(call.functionSignature).toBe(
      "refundBySig(bytes32,uint256,uint256,uint8,bytes32,bytes32)",
    );
    expect(call.data).toBe(
      `0x3bab851e${words(hash, amount, timelock, v27, r, s)}`,
    );
  });
});

describe("rehash calldata", () => {
  it("matches the HTLCErc20 v7 ABI", () => {
    const call = encodeHtlcErc20RehashCallData(htlc, {
      ...terms,
      ...next,
      v: 28,
      r,
      s,
    });
    expect(call.functionSignature).toBe(
      "rehash(bytes32,uint256,address,uint256,bytes32,uint256,uint8,bytes32,bytes32)",
    );
    expect(call.data).toBe(
      `0x5a83bf7f${words(hash, amount, token, timelock, next.newPreimageHash, newTimelock, v28, r, s)}`,
    );
  });

  it("matches the HTLCNative v2 ABI", () => {
    const call = encodeHtlcNativeRehashCallData(htlc, {
      preimageHash: terms.preimageHash,
      amount: terms.amount,
      timelock: terms.timelock,
      ...next,
      v: 28,
      r,
      s,
    });
    expect(call.functionSignature).toBe(
      "rehash(bytes32,uint256,uint256,bytes32,uint256,uint8,bytes32,bytes32)",
    );
    expect(call.data).toBe(
      `0x6eecb3d8${words(hash, amount, timelock, next.newPreimageHash, newTimelock, v28, r, s)}`,
    );
  });

  it("accepts the server's unprefixed hex and bare amounts", () => {
    const prefixed = encodeHtlcErc20RehashCallData(htlc, {
      ...terms,
      ...next,
      v: 28,
      r,
      s,
    });
    const bare = encodeHtlcErc20RehashCallData(htlc, {
      ...terms,
      amount: 100_000_000,
      newPreimageHash: next.newPreimageHash.slice(2),
      newTimelock: next.newTimelock,
      v: 28,
      r: r.slice(2),
      s: s.slice(2),
    });
    expect(bare.data).toBe(prefixed.data);
  });

  it("rejects an r or s that is not exactly 32 bytes", () => {
    for (const bad of [r.slice(0, -2), `${r}00`, `0x${"zz".repeat(32)}`]) {
      expect(() =>
        encodeHtlcErc20RehashCallData(htlc, {
          ...terms,
          ...next,
          v: 28,
          r: bad,
          s,
        }),
      ).toThrow("32 bytes");
      expect(() =>
        encodeHtlcErc20RefundBySigCallData(htlc, {
          ...terms,
          v: 27,
          r,
          s: bad,
        }),
      ).toThrow("32 bytes");
    }
  });

  it("rejects a recovery id that is not a uint8", () => {
    expect(() =>
      encodeHtlcErc20RehashCallData(htlc, { ...terms, ...next, v: 256, r, s }),
    ).toThrow("uint8");
  });
});

describe("depositorIsSender", () => {
  it("is the v7 / native v2 generation, and false for an older server", async () => {
    const { depositorIsSender } = await import("../src/evm/htlc.js");
    expect(depositorIsSender("erc20", 7)).toBe(true);
    expect(depositorIsSender("erc20", 6)).toBe(false);
    expect(depositorIsSender("native", 2)).toBe(true);
    expect(depositorIsSender("native", 1)).toBe(false);
    expect(depositorIsSender(undefined, undefined)).toBe(false);
    expect(depositorIsSender("erc20", undefined)).toBe(false);
  });
});

describe("refundForAsOwnRefund", () => {
  const depositor = "0x7777777777777777777777777777777777777777";
  const claim = "0x3333333333333333333333333333333333333333";
  const pad = (a: string) => a.replace(/^0x/, "").padStart(64, "0");
  const uint = (n: bigint | number) => BigInt(n).toString(16).padStart(64, "0");
  const hash = terms.preimageHash.replace(/^0x/, "");

  it("turns an ERC20 refundFor into the depositor's own refund", () => {
    const refundFor = `0x91fd66ab${hash}${uint(terms.amount)}${pad(terms.token)}${pad(depositor)}${pad(claim)}${uint(terms.timelock)}`;
    expect(
      refundForAsOwnRefund({ to: htlc, data: refundFor }, depositor),
    ).toEqual({
      to: htlc,
      data: `0x36504721${hash}${uint(terms.amount)}${pad(terms.token)}${pad(claim)}${uint(terms.timelock)}`,
    });
  });

  it("turns a native refundFor into the depositor's own refund", () => {
    const refundFor = `0x8dd4ea3e${hash}${uint(terms.amount)}${pad(depositor)}${pad(claim)}${uint(terms.timelock)}`;
    expect(
      refundForAsOwnRefund(
        { to: htlc, data: refundFor },
        depositor.toUpperCase().replace("0X", "0x"),
      ),
    ).toEqual({
      to: htlc,
      data: `0x35cd4ccb${hash}${uint(terms.amount)}${pad(claim)}${uint(terms.timelock)}`,
    });
  });

  it("refuses a lock created by another sender", () => {
    const other = "0x8888888888888888888888888888888888888888";
    const refundFor = `0x91fd66ab${hash}${uint(terms.amount)}${pad(terms.token)}${pad(other)}${pad(claim)}${uint(terms.timelock)}`;
    expect(() =>
      refundForAsOwnRefund({ to: htlc, data: refundFor }, depositor),
    ).toThrow(/created by 0x8888.*only its sender can refund it directly/);
  });

  it("leaves any other call alone", () => {
    const refundTo = { to: htlc, data: "0x29240837" };
    expect(refundForAsOwnRefund(refundTo, depositor)).toBe(refundTo);
  });
});
