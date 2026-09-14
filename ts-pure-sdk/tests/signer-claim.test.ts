import { describe, expect, it, vi } from "vitest";
import type { EvmSigner } from "../src/evm/wallet.js";
import type { GaslessSwapResponse } from "../src/redeem/gasless.js";
import { claimViaSigner } from "../src/redeem/signer-claim.js";
import { buildRedeemAndExecuteTx } from "../src/redeem/userop-claim.js";

/**
 * The wallet-paid claim must broadcast exactly the calldata the sponsored
 * UserOp path builds (the coordinator only checks the EIP-712 signature, not
 * who submits), on the swap's chain, and surface reverts.
 */
const COORDINATOR = "0x1111111111111111111111111111111111111111";
const HTLC = "0x2222222222222222222222222222222222222222";
const WBTC = "0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f";
const USDC = "0xaf88d065e77c8cc2239327c5edb3a432268e5831";
const SERVER = "0x3333333333333333333333333333333333333333";
const DESTINATION = "0x4444444444444444444444444444444444444444";

const swap = {
  id: "swap-1",
  evm_chain_id: 42161,
  evm_htlc_address: HTLC,
  evm_coordinator_address: COORDINATOR,
  evm_expected_sats: 12345,
  evm_refund_locktime: 1_900_000_000,
  evm_htlc_version: 2,
  wbtc_address: WBTC,
  server_evm_address: SERVER,
  target_token: { token_id: USDC, chain: "42161", symbol: "USDC", decimals: 6 },
} as unknown as GaslessSwapResponse;

const secretKey = new Uint8Array(32).map((_, i) => i + 1);
const base = {
  preimage: "11".repeat(32),
  secretKey,
  swap,
  destination: DESTINATION,
  calls: [{ target: USDC, value: 0n, data: "0xdeadbeef" }],
  minAmountOut: 5n,
};

function fakeSigner(overrides: Partial<EvmSigner> = {}): EvmSigner & {
  sent: { to: string; data: string }[];
} {
  const sent: { to: string; data: string }[] = [];
  return {
    address: "0x5555555555555555555555555555555555555555",
    chainId: 42161,
    signTypedData: async () => "0x",
    sendTransaction: async (tx) => {
      sent.push({ to: tx.to, data: tx.data });
      return "0xtxhash";
    },
    waitForReceipt: async () => ({
      status: "success" as const,
      blockNumber: 1n,
      transactionHash: "0xtxhash",
    }),
    getTransaction: async () => ({ to: COORDINATOR, input: "0x", from: "0x" }),
    call: async () => "0x",
    sent,
    ...overrides,
  };
}

describe("claimViaSigner", () => {
  it("broadcasts the same redeemAndExecute calldata as the sponsored path", async () => {
    const signer = fakeSigner();
    const result = await claimViaSigner({ ...base, signer });
    const expected = buildRedeemAndExecuteTx(base);
    expect(signer.sent).toEqual([expected]);
    expect(signer.sent[0].to).toBe(COORDINATOR);
    expect(result).toMatchObject({
      id: "swap-1",
      status: "clientredeemed",
      txHash: "0xtxhash",
    });
  });

  it("refuses a wallet on the wrong chain before touching it", async () => {
    const signer = fakeSigner({ chainId: 1 });
    await expect(claimViaSigner({ ...base, signer })).rejects.toThrow(
      /chain 1.*chain 42161/,
    );
    expect(signer.sent).toEqual([]);
  });

  it("simulates first and surfaces a revert without sending", async () => {
    const signer = fakeSigner({
      call: vi.fn(async () => {
        throw new Error("execution reverted: InvalidPreimage");
      }),
    });
    await expect(claimViaSigner({ ...base, signer })).rejects.toThrow(
      /InvalidPreimage/,
    );
    expect(signer.sent).toEqual([]);
  });

  it("reports a reverted transaction", async () => {
    const signer = fakeSigner({
      waitForReceipt: async () => ({
        status: "reverted" as const,
        blockNumber: 1n,
        transactionHash: "0xtxhash",
      }),
    });
    await expect(claimViaSigner({ ...base, signer })).rejects.toThrow(
      /Claim transaction reverted/,
    );
  });
});
