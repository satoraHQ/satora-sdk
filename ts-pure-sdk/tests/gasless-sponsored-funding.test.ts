import { afterEach, describe, expect, it, vi } from "vitest";
import { deriveEvmAddress } from "../src/evm/signing.js";
import { Client, InMemorySwapStorage } from "../src/index.js";
import {
  COORDINATOR,
  FUND_TXID,
  fundingTerms,
  HASH_LOCK,
  REAL_LOG,
  SWAP_ID,
  storedSwap,
  swapResponse,
} from "./evm-funding-fixture.js";

const receiptRpc = vi.hoisted(() => vi.fn());
vi.mock("viem", async (importOriginal) => ({
  ...(await importOriginal<typeof import("viem")>()),
  createPublicClient: () => ({ getTransactionReceipt: receiptRpc }),
}));

const USER_OP = `0x${"0e".repeat(32)}` as const;
const FUNDING_TX = `0x${"f0".repeat(32)}` as const;
const WALLET = "0x90f79bf6eb2c4f870365e785982e1f101e93b906";
/** The fixture's per-swap key (`storedSwap().secretKey`) and its address. */
const PER_SWAP_KEY = "01".repeat(32);
const AA = {
  bundlerUrl: "http://bundler.test",
  paymasterPolicyId: "test-policy",
};

/**
 * A client holding an unfunded gasless swap on Arbitrum. The server is a
 * fetch mock that fails every request: the sponsored path must neither hit
 * the legacy relay nor need any endpoint beyond `getSwap`, which is spied.
 */
async function clientWith(
  options: {
    aa?: boolean;
    chainId?: number;
    depositorOf?: (sdkAddress: string) => string | null;
    evmFundTxid?: string;
  } = {},
) {
  const { aa = true, chainId = 42161 } = options;
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    throw new Error(
      `unexpected request ${input instanceof Request ? input.url : input}`,
    );
  });
  vi.stubGlobal("fetch", fetchMock);
  const storage = new InMemorySwapStorage();
  let builder = Client.builder().withSwapStorage(storage);
  if (aa) builder = builder.withAa(AA);
  const client = await builder.build();
  const depositor = (options.depositorOf ?? ((sdk) => sdk))(
    client.getEvmAddress(),
  );
  const response = {
    ...swapResponse({
      gasless: true,
      clientEvmAddress: depositor,
    }),
    evm_chain_id: chainId,
  } as ReturnType<typeof swapResponse>;
  await storage.store(
    storedSwap({
      evmFundTxid: options.evmFundTxid,
      evmCoordinatorAddress: undefined,
      evmFundingTerms: fundingTerms({ chainId }),
      response,
    }),
  );
  vi.spyOn(client, "getSwap").mockResolvedValue(response as never);
  return { client, storage, fetchMock };
}

function sponsorable(client: Client) {
  return vi.spyOn(client.cctpInbound, "submitBalanceUserOp").mockResolvedValue({
    userOpHash: USER_OP,
    transactionHash: FUNDING_TX,
    smartAccountAddress: client.getEvmAddress() as `0x${string}`,
  });
}

function relayNeverUsed(fetchMock: ReturnType<typeof vi.fn>) {
  expect(
    fetchMock.mock.calls.filter(([input]) =>
      String(input instanceof Request ? input.url : input).includes(
        "fund-gasless",
      ),
    ),
  ).toHaveLength(0);
}

afterEach(() => {
  receiptRpc.mockReset();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("fundSwapGasless funds Arbitrum swaps as a sponsored UserOp", () => {
  it("submits the funding itself instead of the legacy relay, and records it", async () => {
    const { client, storage, fetchMock } = await clientWith();
    const submit = sponsorable(client);

    await expect(client.fundSwapGasless(SWAP_ID)).resolves.toEqual({
      txHash: FUNDING_TX,
    });

    const args = submit.mock.calls[0][0];
    expect(args.swapId).toBe(SWAP_ID);
    // The swap's own hash and recorded terms bound the batch before signing.
    expect(args.expected.hashLock).toBe(HASH_LOCK);
    expect(args.expected.terms.chainId).toBe(42161);
    // The spend is capped at the deposit the user was asked for.
    expect(args.expectedMaxSourceAmount).toBe(15658166n);
    // The depositor key owns the Kernel account the deposit sits in.
    expect(args.signer.address.toLowerCase()).toBe(
      client.getEvmAddress().toLowerCase(),
    );
    relayNeverUsed(fetchMock);
    expect(await storage.get(SWAP_ID)).toMatchObject({
      evmFundTxid: FUNDING_TX,
      evmCoordinatorAddress: COORDINATOR,
    });
  });

  it("passes the funding pins through to the batch", async () => {
    const { client } = await clientWith();
    const submit = sponsorable(client);
    const funding = { maxRelayFee: 5n };

    await client.fundSwapGasless(SWAP_ID, { funding });

    expect(submit.mock.calls[0][0].expected.pins).toEqual(funding);
  });

  it("funds a legacy swap from its per-swap depositor key", async () => {
    const legacyDepositor = deriveEvmAddress(PER_SWAP_KEY);
    const { client } = await clientWith({ depositorOf: () => legacyDepositor });
    const submit = sponsorable(client);

    await client.fundSwapGasless(SWAP_ID);

    expect(submit.mock.calls[0][0].signer.address.toLowerCase()).toBe(
      legacyDepositor.toLowerCase(),
    );
  });

  it("refuses a depositor this SDK does not control", async () => {
    const { client, fetchMock } = await clientWith({
      depositorOf: () => WALLET,
    });
    const submit = sponsorable(client);

    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      /not an SDK-controlled account/,
    );
    expect(submit).not.toHaveBeenCalled();
    relayNeverUsed(fetchMock);
  });

  it("requires AA config instead of falling back to the relay", async () => {
    const { client, fetchMock } = await clientWith({ aa: false });

    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(/withAa/);
    relayNeverUsed(fetchMock);
  });

  it("surfaces a sponsorship failure instead of falling back to the relay", async () => {
    const { client, storage, fetchMock } = await clientWith();
    vi.spyOn(client.cctpInbound, "submitBalanceUserOp").mockRejectedValue(
      new Error("paymaster refused to sponsor the UserOp"),
    );

    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      /paymaster refused/,
    );
    relayNeverUsed(fetchMock);
    // Nothing recorded: the swap stays fundable once the failure is fixed.
    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBeUndefined();
  });

  it("answers a recorded funding instead of funding twice", async () => {
    const { client, fetchMock } = await clientWith({ evmFundTxid: FUND_TXID });
    receiptRpc.mockResolvedValue({
      status: "success",
      logs: [REAL_LOG],
      transactionHash: FUND_TXID,
    });
    const submit = sponsorable(client);

    await expect(client.fundSwapGasless(SWAP_ID)).resolves.toEqual({
      txHash: FUND_TXID,
    });
    expect(submit).not.toHaveBeenCalled();
    relayNeverUsed(fetchMock);
  });

  it("refuses a swap whose recorded terms are not on the sponsored chain", async () => {
    // The server says Arbitrum but this client recorded the lock on another
    // chain at creation: nothing is signed for the mismatch.
    const { client } = await clientWith();
    const submit = sponsorable(client);
    const storage = client.swapStorage as InMemorySwapStorage;
    const stored = await storage.get(SWAP_ID);
    await storage.store({
      ...(stored as NonNullable<typeof stored>),
      evmFundingTerms: fundingTerms({ chainId: 137 }),
    });

    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      /locks on chain 137/,
    );
    expect(submit).not.toHaveBeenCalled();
  });
});

describe("gaslessFundingPlan names the funding path before a deposit is requested", () => {
  it("is sponsored on Arbitrum with AA config", async () => {
    const { client } = await clientWith();
    expect(client.gaslessFundingPlan(42161)).toEqual({ mode: "sponsored" });
  });

  it("throws on Arbitrum without AA config", async () => {
    const { client } = await clientWith({ aa: false });
    expect(() => client.gaslessFundingPlan(42161)).toThrow(/withAa/);
  });

  it("stays on the relay for chains without a sponsorship policy", async () => {
    const { client } = await clientWith();
    expect(client.gaslessFundingPlan(1)).toEqual({ mode: "relay" });
    expect(client.gaslessFundingPlan(137)).toEqual({ mode: "relay" });
  });
});

describe("fundSwapGasless keeps the relay on chains without sponsorship", () => {
  it("does not submit a sponsored UserOp for a Polygon swap", async () => {
    // The relay body itself is covered by the existing gasless relay tests;
    // here the server answers the relay routes and the sponsored path must
    // stay untouched.
    const { client, fetchMock } = await clientWith({ chainId: 137 });
    const submit = sponsorable(client);
    // The Permit2 params request is the relay path's first step; failing
    // there proves the routing without mocking the whole relay exchange.
    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      /Failed to get Permit2 funding data|unexpected request/,
    );
    expect(submit).not.toHaveBeenCalled();
    expect(
      fetchMock.mock.calls.some(([input]) =>
        String(input instanceof Request ? input.url : input).includes(
          "swap-and-lock-calldata-permit2",
        ),
      ),
    ).toBe(true);
  });
});

describe("sponsored funding submission guard", () => {
  it("rejects overlapping funding calls before a second batch is built", async () => {
    const { client } = await clientWith();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const submit = vi
      .spyOn(client.cctpInbound, "submitBalanceUserOp")
      .mockImplementation(async (args) => {
        await args.onSigned!(USER_OP);
        await gate;
        return {
          userOpHash: USER_OP,
          transactionHash: FUNDING_TX,
          smartAccountAddress: WALLET,
        };
      });
    const first = client.fundSwapGasless(SWAP_ID);
    await vi.waitFor(() => expect(submit).toHaveBeenCalledOnce());
    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      "already in progress",
    );
    release();
    await first;
    expect(submit).toHaveBeenCalledOnce();
  });

  it.each([
    "send response lost",
    "receipt timed out",
  ])("persists before %s and reconciles after a client restart", async (reason) => {
    const { client, storage } = await clientWith({
      depositorOf: () => deriveEvmAddress(PER_SWAP_KEY),
    });
    vi.spyOn(client.cctpInbound, "submitBalanceUserOp").mockImplementation(
      async (args) => {
        await args.onSigned!(USER_OP);
        expect((await storage.get(SWAP_ID))?.evmFundingUserOpHash).toBe(
          USER_OP,
        );
        throw new Error(reason);
      },
    );
    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(reason);
    const restarted = await Client.builder()
      .withSwapStorage(storage)
      .withAa(AA)
      .build();
    vi.spyOn(restarted, "getSwap").mockResolvedValue(
      (await storage.get(SWAP_ID))!.response,
    );
    const submit = sponsorable(restarted);
    const receipt = vi.fn().mockRejectedValue(new Error("not found"));
    vi.spyOn(
      restarted.cctpInbound,
      "createSmartAccountClient",
    ).mockResolvedValue({
      client: { getUserOperationReceipt: receipt },
    } as never);
    await expect(restarted.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      "outcome unknown",
    );
    expect(submit).not.toHaveBeenCalled();
    receipt.mockResolvedValue({
      success: true,
      receipt: { transactionHash: FUNDING_TX },
    });
    await expect(restarted.fundSwapGasless(SWAP_ID)).resolves.toEqual({
      txHash: FUNDING_TX,
    });
    expect(submit).not.toHaveBeenCalled();
    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(FUNDING_TX);
  });

  it("allows a fresh attempt only after a confirmed revert", async () => {
    const { client, storage } = await clientWith();
    await storage.store({
      ...(await storage.get(SWAP_ID))!,
      evmFundingUserOpHash: USER_OP,
    });
    vi.spyOn(client.cctpInbound, "createSmartAccountClient").mockResolvedValue({
      client: {
        getUserOperationReceipt: vi.fn().mockResolvedValue({
          success: false,
          receipt: { transactionHash: FUNDING_TX },
        }),
      },
    } as never);
    const submit = sponsorable(client);
    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow("reverted");
    expect(submit).not.toHaveBeenCalled();
    expect((await storage.get(SWAP_ID))?.evmFundingUserOpHash).toBeUndefined();
    await expect(client.fundSwapGasless(SWAP_ID)).resolves.toEqual({
      txHash: FUNDING_TX,
    });
    expect(submit).toHaveBeenCalledOnce();
  });
});

describe("recorded funding reconciliation", () => {
  it.each([
    "reverted",
    "cancelled",
  ])("retries a confirmed %s transaction", async (outcome) => {
    const { client, storage } = await clientWith({ evmFundTxid: FUND_TXID });
    receiptRpc.mockResolvedValue({
      status: outcome === "reverted" ? "reverted" : "success",
      logs: [],
      transactionHash: FUND_TXID,
    });
    const submit = sponsorable(client);
    await expect(client.fundSwapGasless(SWAP_ID)).resolves.toEqual({
      txHash: FUNDING_TX,
    });
    expect(submit).toHaveBeenCalledOnce();
    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(FUNDING_TX);
  });

  it.each([
    "pending",
    "RPC unavailable",
  ])("keeps %s funding blocked", async (reason) => {
    const { client, storage } = await clientWith({ evmFundTxid: FUND_TXID });
    receiptRpc.mockRejectedValue(new Error(reason));
    const submit = sponsorable(client);
    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      "outcome is unknown",
    );
    expect(submit).not.toHaveBeenCalled();
    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(FUND_TXID);
  });

  it("does not retry when a lock under this hash has unexpected terms", async () => {
    const { client } = await clientWith({ evmFundTxid: FUND_TXID });
    receiptRpc.mockResolvedValue({
      status: "success",
      logs: [{ ...REAL_LOG, address: WALLET }],
      transactionHash: FUND_TXID,
    });
    const submit = sponsorable(client);
    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      "unexpected lock",
    );
    expect(submit).not.toHaveBeenCalled();
  });
});

describe("sponsorship policy requirement", () => {
  it.each([
    undefined,
    "",
    "   ",
  ])("rejects missing/blank policy %s before submitting funding", async (policy) => {
    const client = await Client.builder()
      .withAa({ bundlerUrl: AA.bundlerUrl, paymasterPolicyId: policy })
      .build();
    expect(() => client.gaslessFundingPlan(42161)).toThrow("paymasterPolicyId");
    expect(client.gaslessFundingPlan(137)).toEqual({ mode: "relay" });
  });
  it("labels explicit account-paid tests as self-paid", async () => {
    const client = await Client.builder()
      .withAa({ bundlerUrl: AA.bundlerUrl, allowSelfPaidFunding: true })
      .build();
    expect(client.gaslessFundingPlan(42161)).toEqual({ mode: "self-paid" });
  });
  it("uses sponsorship when a policy is present even with the test opt-in", async () => {
    const client = await Client.builder()
      .withAa({ ...AA, allowSelfPaidFunding: true })
      .build();
    expect(client.gaslessFundingPlan(42161)).toEqual({ mode: "sponsored" });
  });
});
