import { describe, expect, it } from "vitest";
import { simulateTransaction } from "../src/evm/wallet.js";
import { type EvmSigner, SimulationRevertError } from "../src/index.js";

function signerFailingWith(message: string): EvmSigner {
  return {
    address: "0x6d7484c98c8197e4afa56bc2b8c0204445f291bc",
    chainId: 42161,
    call: async () => {
      throw new Error(message);
    },
  } as unknown as EvmSigner;
}

describe("simulateTransaction", () => {
  it("throws a typed error carrying the revert reason", async () => {
    const signer = signerFailingWith(
      "Execution reverted with reason: Too little received",
    );
    const attempt = simulateTransaction(
      signer,
      { to: "0xdb76f29f1d04d1202f56a34549869b070dfcbff0", data: "0x" },
      "Funding transaction",
    );
    await expect(attempt).rejects.toBeInstanceOf(SimulationRevertError);
    await expect(attempt).rejects.toMatchObject({
      reason: "Too little received",
      message: "Funding transaction would revert: Too little received",
    });
  });

  it("falls back to the raw message when no reason is parseable", async () => {
    const signer = signerFailingWith("execution reverted");
    await expect(
      simulateTransaction(signer, { to: "0x", data: "0x" }, "Refund"),
    ).rejects.toMatchObject({ reason: "execution reverted" });
  });

  it("resolves when the call succeeds", async () => {
    const signer = {
      address: "0x6d7484c98c8197e4afa56bc2b8c0204445f291bc",
      chainId: 42161,
      call: async () => "0x",
    } as unknown as EvmSigner;
    await expect(
      simulateTransaction(signer, { to: "0x", data: "0x" }, "Funding"),
    ).resolves.toBeUndefined();
  });
});
