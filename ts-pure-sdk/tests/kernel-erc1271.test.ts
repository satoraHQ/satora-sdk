import { describe, expect, it } from "vitest";
import {
  isKernelDelegation,
  KERNEL_DELEGATION_TARGET,
  kernelErc1271Digest,
  parseEip7702Delegation,
  wrapKernelErc1271Signature,
} from "../src/evm/kernel.js";

describe("Kernel V3.3 ERC-1271 envelope", () => {
  // Cross-SDK vector: same inputs as rust-sdk `erc1271_wrapped_digest`
  // tests, expected value computed independently with `cast`.
  it("matches the pinned digest for inner hash / account / chain", () => {
    const inner =
      "0x1234567890123456789012345678901234567890123456789012345678901234";
    const account = "0x1111111111111111111111111111111111111111";
    expect(kernelErc1271Digest(inner, account, 42161)).toBe(
      "0x40167c9b20ce7aef51b92d380a861f31753b1789ace2be8b3f76c2c1331fcee9",
    );
  });

  it("is sensitive to each input", () => {
    const inner =
      "0x1234567890123456789012345678901234567890123456789012345678901234";
    const account = "0x1111111111111111111111111111111111111111";
    const base = kernelErc1271Digest(inner, account, 42161);
    expect(
      kernelErc1271Digest(
        "0xabcdef1234567890123456789012345678901234567890123456789012345678",
        account,
        42161,
      ),
    ).not.toBe(base);
    expect(
      kernelErc1271Digest(
        inner,
        "0x2222222222222222222222222222222222222222",
        42161,
      ),
    ).not.toBe(base);
    expect(kernelErc1271Digest(inner, account, 1)).not.toBe(base);
  });

  it("prepends the sudo-mode selector to a 65-byte signature", () => {
    const sig = `0x${"bb".repeat(65)}`;
    const wrapped = wrapKernelErc1271Signature(sig);
    expect(wrapped).toBe(`0x00${"bb".repeat(65)}`);
    expect((wrapped.length - 2) / 2).toBe(66);
    expect(() => wrapKernelErc1271Signature(`0x${"bb".repeat(64)}`)).toThrow();
  });

  it("recognises an EIP-7702 delegation to Kernel and nothing else", () => {
    const kernelCode = `0xef0100${KERNEL_DELEGATION_TARGET.slice(2)}`;
    expect(parseEip7702Delegation(kernelCode)).toBe(
      KERNEL_DELEGATION_TARGET.toLowerCase(),
    );
    expect(isKernelDelegation(kernelCode)).toBe(true);
    expect(
      isKernelDelegation(kernelCode.toUpperCase().replace("0X", "0x")),
    ).toBe(true);
    // Simple7702Account-style delegation to another implementation
    expect(isKernelDelegation(`0xef0100${"ab".repeat(20)}`)).toBe(false);
    // plain EOA / undeployed
    expect(isKernelDelegation("0x")).toBe(false);
    expect(isKernelDelegation(undefined)).toBe(false);
    // regular contract bytecode
    expect(isKernelDelegation("0x6080604052")).toBe(false);
  });
});
