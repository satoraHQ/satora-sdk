import { recoverAddress } from "viem";
import { describe, expect, it } from "vitest";
import {
  encodeExecuteAndCreateWithPermit2,
  normalizePermit2Signature,
} from "../src/evm/coordinator.js";

const signature =
  "0xa9839ee56ddd6542c1d5307bfe0118f8aa1e30afac24b6d93ebc24b82683940a05507c18a56892cc4df3afc4b6c511e5b21e139241b19d9608f973faa8d287d500";
describe("Permit2 EOA signature compatibility", () => {
  it("recovers the affected wallet from the production digest after normalization", async () => {
    const normalized = normalizePermit2Signature(signature, "0x");
    expect(normalized).toBe(signature.slice(0, -2) + "1b");
    expect(
      await recoverAddress({
        hash: "0xb28e29a77d434aaec17e55235831e2d3352f430dc52a21b4693341592d5ca2f3",
        signature: normalized as `0x${string}`,
      }),
    ).toBe("0xf5500f85A886F916B26a83bD8858F03f4BFC685b");
  });
  it.each([
    ["00", "1b"],
    ["01", "1c"],
  ])("normalizes parity %s", (v, expected) => {
    expect(normalizePermit2Signature(signature.slice(0, -2) + v, "0x")).toBe(
      signature.slice(0, -2) + expected,
    );
  });
  it.each(["1b", "1c"])("preserves valid recovery byte %s", (v) => {
    const valid = signature.slice(0, -2) + v;
    expect(normalizePermit2Signature(valid, "0x")).toBe(valid);
  });
  it("preserves EIP-2098 signatures", () => {
    const compact = signature.slice(0, -2);
    expect(normalizePermit2Signature(compact, "0x")).toBe(compact);
  });
  it.each([
    "0x6000",
    "0xef01000000000000000000000000000000000000000001",
    undefined,
  ])("preserves opaque signatures with code %s", (code) => {
    for (const opaque of [signature, signature.slice(0, -2) + "01", "0x1234"]) {
      expect(normalizePermit2Signature(opaque, code)).toBe(opaque);
    }
  });
  it("diagnoses invalid EOA signatures", () => {
    expect(() =>
      normalizePermit2Signature(signature.slice(0, -2) + "02", "0x"),
    ).toThrow("recovery byte");
    expect(() => normalizePermit2Signature("0x1234", "0x")).toThrow(
      "64 or 65 bytes",
    );
  });
});

it.each([
  "0x",
  "0x6000",
  undefined,
])("encodes the appropriate signature with owner code %s", (depositorCode) => {
  const address = "0xf5500f85A886F916B26a83bD8858F03f4BFC685b";
  const { data } = encodeExecuteAndCreateWithPermit2(address, {
    calls: [],
    preimageHash: "0x" + "11".repeat(32),
    token: address,
    claimAddress: address,
    timelock: 1,
    depositor: address,
    sourceToken: address,
    sourceAmount: 98491623n,
    nonce: 1n,
    deadline: 2n,
    signature,
    depositorCode,
  });
  const args = data.slice(10);
  const offset = Number(BigInt("0x" + args.slice(640, 704))) * 2;
  expect(BigInt("0x" + args.slice(offset, offset + 64))).toBe(65n);
  expect("0x" + args.slice(offset + 64, offset + 194)).toBe(
    depositorCode === "0x" ? signature.slice(0, -2) + "1b" : signature,
  );
});
