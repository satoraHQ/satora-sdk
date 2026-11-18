import { beforeEach, expect, it, vi } from "vitest";
import { fundingTerms, HASH_LOCK } from "./evm-funding-fixture.js";

const mocks = vi.hoisted(() => ({
  prepare: vi.fn(),
  sign: vi.fn(),
  request: vi.fn(),
  wait: vi.fn(),
  hash: vi.fn(),
}));
vi.mock("../src/cctp-inbound/smartAccount.js", () => ({
  createSwapSmartAccountClient: async () => ({
    client: {
      prepareUserOperation: mocks.prepare,
      request: mocks.request,
      waitForUserOperationReceipt: mocks.wait,
    },
    account: {
      signUserOperation: mocks.sign,
      entryPoint: { address: `0x${"11".repeat(20)}`, version: "0.7" },
    },
    accountAddress: `0x${"22".repeat(20)}`,
  }),
}));
vi.mock("../src/cctp-inbound/userOp.js", () => ({
  PERMIT2_ADDRESS: `0x${"33".repeat(20)}`,
  buildCctpInboundBatch: async () => ({ calls: [] }),
}));
vi.mock("viem/account-abstraction", () => ({
  getUserOperationHash: mocks.hash,
  formatUserOperationRequest: (op: unknown) => op,
}));
vi.mock("viem", async (importOriginal) => ({
  ...(await importOriginal<typeof import("viem")>()),
  createPublicClient: () => ({ readContract: async () => 0n }),
  http: vi.fn(),
}));

import { submitCctpInboundBalanceUserOp } from "../src/cctp-inbound/submit.js";

const HASH = `0x${"44".repeat(32)}` as const;
const context = {
  apiClient: { GET: async () => ({ data: { source_amount: "1" } }) },
  aa: { bundlerUrl: "http://bundler.test" },
} as never;
const params = {
  swapId: "test",
  signer: {} as never,
  expected: { terms: fundingTerms(), hashLock: HASH_LOCK },
  noWait: true,
};
beforeEach(() => {
  vi.resetAllMocks();
  mocks.prepare.mockResolvedValue({ nonce: 1n });
  mocks.sign.mockResolvedValue("0x01");
  mocks.hash.mockReturnValue(HASH);
  mocks.request.mockResolvedValue(HASH);
});
it("persists the locally computed signed hash before submission, even when the response is lost", async () => {
  let persisted = false;
  mocks.request.mockImplementation(async () => {
    expect(persisted).toBe(true);
    throw new Error("lost response");
  });
  await expect(
    submitCctpInboundBalanceUserOp(context, {
      ...params,
      onSigned: async (hash) => {
        expect(hash).toBe(HASH);
        persisted = true;
      },
    }),
  ).rejects.toThrow("lost response");
  expect(mocks.request).toHaveBeenCalledOnce();
  expect(mocks.request.mock.calls[0][1]).toEqual({ retryCount: 0 });
});
it("does not submit if persisting the signed hash fails", async () => {
  await expect(
    submitCctpInboundBalanceUserOp(context, {
      ...params,
      onSigned: async () => {
        throw new Error("storage unavailable");
      },
    }),
  ).rejects.toThrow("storage unavailable");
  expect(mocks.request).not.toHaveBeenCalled();
});
it("does not submit if the signed hash cannot be computed", async () => {
  mocks.hash.mockImplementation(() => {
    throw new Error("invalid operation");
  });
  await expect(
    submitCctpInboundBalanceUserOp(context, { ...params, onSigned: vi.fn() }),
  ).rejects.toThrow("invalid operation");
  expect(mocks.request).not.toHaveBeenCalled();
});
