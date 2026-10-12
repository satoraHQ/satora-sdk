import type { Hex } from "viem";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The chain-facing pieces of recovery: the Kernel account client and the
// Arbitrum reads. Mocked so the tests exercise the real `recoverCctpInbound`
// and how it treats a UserOp that was included but reverted.
const sendUserOperation = vi.fn();
const waitForUserOperationReceipt = vi.fn();
const readContract = vi.fn();

vi.mock("../src/cctp-inbound/smartAccount.js", () => ({
  createSwapSmartAccountClient: async () => ({
    accountAddress: ACCOUNT,
    client: { sendUserOperation, waitForUserOperationReceipt },
  }),
}));
vi.mock("viem", async (importOriginal) => ({
  ...(await importOriginal<typeof import("viem")>()),
  createPublicClient: () => ({ readContract }),
}));

import { recoverCctpInbound } from "../src/cctp-inbound/recover.js";
import type { EvmSigner } from "../src/evm/wallet.js";

const ACCOUNT = "0x00000000000000000000000000000000000000aa";
const RECIPIENT = "0x00000000000000000000000000000000000000bb";
const RECEIVE_OP = `0x${"01".repeat(32)}` as Hex;
const SWEEP_OP = `0x${"02".repeat(32)}` as Hex;
const SWEEP_TX = `0x${"03".repeat(32)}` as Hex;

/** A CCTPv2 message whose `destinationCaller` is the Kernel account. */
const MESSAGE =
  `0x${"00".repeat(120)}${ACCOUNT.slice(2)}${"00".repeat(20)}` as Hex;

function recover(onProgress = vi.fn()) {
  return recoverCctpInbound(
    { aa: { bundlerUrl: "http://bundler.test" } },
    {
      signer: {} as EvmSigner,
      burnTxHash: `0x${"b0".repeat(32)}`,
      sourceChain: "Optimism",
      recipient: RECIPIENT,
      attestation: { message: MESSAGE, attestation: "0x02" },
      onProgress,
    },
  );
}

/** Answers `usedNonces` with `nonceUsed` and `balanceOf` with 1 USDC. */
function chainWith(nonceUsed: boolean) {
  readContract.mockImplementation(async ({ functionName }) =>
    functionName === "usedNonces" ? (nonceUsed ? 1n : 0n) : 1_000_000n,
  );
}

beforeEach(() => {
  sendUserOperation
    .mockResolvedValueOnce(RECEIVE_OP)
    .mockResolvedValueOnce(SWEEP_OP);
});

afterEach(() => {
  vi.resetAllMocks();
});

describe("recoverCctpInbound", () => {
  it("sweeps the minted USDC to the recipient", async () => {
    chainWith(false);
    waitForUserOperationReceipt.mockResolvedValue({
      success: true,
      receipt: { transactionHash: SWEEP_TX },
    });

    const result = await recover();

    expect(result.recoveredAmount).toBe(1_000_000n);
    expect(result.sweepTxHash).toBe(SWEEP_TX);
  });

  it("fails instead of reporting success when the sweep reverted", async () => {
    chainWith(true);
    sendUserOperation.mockReset().mockResolvedValueOnce(SWEEP_OP);
    waitForUserOperationReceipt.mockResolvedValue({
      success: false,
      reason: "ERC20: transfer amount exceeds balance",
      receipt: { transactionHash: SWEEP_TX },
    });
    const onProgress = vi.fn();

    await expect(recover(onProgress)).rejects.toThrow(
      /sweep UserOp .* reverted on-chain: ERC20: transfer amount exceeds balance/,
    );
    expect(onProgress).not.toHaveBeenCalledWith(
      expect.objectContaining({ phase: "done" }),
    );
  });

  it("does not sweep after receiveMessage reverted", async () => {
    chainWith(false);
    waitForUserOperationReceipt.mockResolvedValue({ success: false });

    await expect(recover()).rejects.toThrow(
      /receiveMessage UserOp .* reverted on-chain/,
    );
    expect(sendUserOperation).toHaveBeenCalledTimes(1);
  });
});
