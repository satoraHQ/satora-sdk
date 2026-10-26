import { describe, expect, it, vi } from "vitest";
import { type EvmSigner, gasLimitFor } from "../src/evm/wallet.js";

const TX = { to: "0x1111111111111111111111111111111111111111", data: "0xdead" };
const ADDRESS = "0x2222222222222222222222222222222222222222";

function signerWith(overrides: Partial<EvmSigner>): EvmSigner {
  return {
    address: ADDRESS,
    chainId: 42161,
    signTypedData: async () => "0x",
    sendTransaction: async () => "0x",
    waitForReceipt: async () => {
      throw new Error("unused");
    },
    getTransaction: async () => {
      throw new Error("unused");
    },
    call: async () => "0x",
    ...overrides,
  };
}

describe("gasLimitFor", () => {
  it("adds a 30% margin to the signer's estimate, sent from the signer", async () => {
    // A two-hop Permit2 funding on Arbitrum estimated 503,765: the old static
    // 500k cap burned the whole limit on an out-of-gas revert.
    const estimateGas = vi.fn(async () => 503_765n);
    const signer = signerWith({ estimateGas });
    expect(await gasLimitFor(signer, TX, 500_000n)).toBe(654_894n);
    expect(estimateGas).toHaveBeenCalledWith({ ...TX, from: ADDRESS });
  });

  it("falls back to the static limit when the signer cannot estimate", async () => {
    expect(await gasLimitFor(signerWith({}), TX, 500_000n)).toBe(500_000n);
  });

  it("falls back to the static limit when estimation throws", async () => {
    const signer = signerWith({
      estimateGas: async () => {
        throw new Error("execution reverted");
      },
    });
    expect(await gasLimitFor(signer, TX, 500_000n)).toBe(500_000n);
  });
});
