import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The chain-facing primitives of the CCTP-inbound flow. Mocked so the tests
// exercise the real `cctpFundSwap` and `Client.fundSwap` wiring: which signer
// burns, which one owns the settling Kernel account, and when the burn is
// recorded or resumed.
const approveAndBurn = vi.fn();
const fetchAttestation = vi.fn();
const createSwapSmartAccountClient = vi.fn();
const submitCctpInboundUserOp = vi.fn();
const recoverCctpInbound = vi.fn();

vi.mock("../src/cctp-inbound/approveAndBurn.js", () => ({
  approveAndBurn: (...args: unknown[]) => approveAndBurn(...args),
}));
vi.mock("../src/cctp/attestation.js", () => ({
  fetchAttestation: (...args: unknown[]) => fetchAttestation(...args),
}));
vi.mock("../src/cctp-inbound/smartAccount.js", () => ({
  createSwapSmartAccountClient: (...args: unknown[]) =>
    createSwapSmartAccountClient(...args),
}));
vi.mock("../src/cctp-inbound/submit.js", () => ({
  submitCctpInboundUserOp: (...args: unknown[]) =>
    submitCctpInboundUserOp(...args),
  submitCctpInboundBalanceUserOp: vi.fn(),
}));
vi.mock("../src/cctp-inbound/recover.js", () => ({
  recoverCctpInbound: (...args: unknown[]) => recoverCctpInbound(...args),
}));

import { cctpFundSwap } from "../src/cctp-inbound/fundSwap.js";
import type { EvmSigner } from "../src/evm/wallet.js";
import {
  Client,
  InMemorySwapStorage,
  InMemoryWalletStorage,
} from "../src/index.js";
import { SWAP_ID, storedSwap, swapResponse } from "./evm-funding-fixture.js";

const WALLET = "0x90f79bf6eb2c4f870365e785982e1f101e93b906";
const OPTIMISM_USDC = "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85";
const BURN = `0x${"b0".repeat(32)}` as const;
const USER_OP = `0x${"0e".repeat(32)}` as const;
const SETTLE_TX = `0x${"5e".repeat(32)}` as const;
const AA = { bundlerUrl: "http://bundler.test" };

/** The user's browser wallet on Optimism: sends txs, cannot sign 7702. */
function walletOnOptimism(): EvmSigner {
  return {
    address: WALLET,
    chainId: 10,
    signTypedData: () => Promise.reject(new Error("not needed")),
    sendTransaction: () => Promise.reject(new Error("not needed")),
    waitForReceipt: async (hash) => ({
      status: "success",
      transactionHash: hash,
      blockNumber: 1n,
      logs: [],
    }),
    getTransaction: () => Promise.reject(new Error("not needed")),
    call: () => Promise.reject(new Error("not needed")),
  } as EvmSigner;
}

beforeEach(() => {
  approveAndBurn.mockResolvedValue({ burnTxHash: BURN });
  fetchAttestation.mockResolvedValue({ message: "0x01", attestation: "0x02" });
  createSwapSmartAccountClient.mockImplementation(
    async ({ signer }: { signer: EvmSigner }) => ({
      accountAddress: signer.address,
    }),
  );
  submitCctpInboundUserOp.mockResolvedValue({
    userOpHash: USER_OP,
    transactionHash: SETTLE_TX,
  });
});

afterEach(() => {
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe("cctpFundSwap", () => {
  const context = { apiClient: {} as never, aa: AA };
  const owner = { ...walletOnOptimism(), address: `0x${"0a".repeat(20)}` };

  it("burns with the wallet and settles with the separate owner", async () => {
    const wallet = walletOnOptimism();
    const result = await cctpFundSwap(context, {
      swapId: SWAP_ID,
      signer: wallet,
      ownerSigner: owner,
      amount: 1_000_000n,
      maxFee: 100n,
    });

    // The owner's account receives the mint, so it is pinned on the burn.
    expect(approveAndBurn).toHaveBeenCalledWith(
      expect.objectContaining({
        signer: wallet,
        smartAccountAddress: owner.address,
      }),
    );
    expect(submitCctpInboundUserOp).toHaveBeenCalledWith(
      context,
      expect.objectContaining({ signer: owner }),
    );
    expect(result.smartAccountAddress).toBe(owner.address);
  });

  it("uses the source signer as owner when none is given", async () => {
    const wallet = walletOnOptimism();
    await cctpFundSwap(context, {
      swapId: SWAP_ID,
      signer: wallet,
      amount: 1_000_000n,
      maxFee: 100n,
    });

    expect(createSwapSmartAccountClient).toHaveBeenCalledWith(
      expect.objectContaining({ signer: wallet }),
    );
    expect(submitCctpInboundUserOp).toHaveBeenCalledWith(
      context,
      expect.objectContaining({ signer: wallet }),
    );
  });

  it("reports the burn before waiting for its receipt", async () => {
    const order: string[] = [];
    const wallet = walletOnOptimism();
    wallet.waitForReceipt = async (hash) => {
      order.push("receipt");
      return { status: "success", transactionHash: hash, blockNumber: 1n };
    };

    await cctpFundSwap(context, {
      swapId: SWAP_ID,
      signer: wallet,
      amount: 1_000_000n,
      maxFee: 100n,
      onBurn: (hash) => {
        order.push(`burn ${hash}`);
      },
    });

    expect(order).toEqual([`burn ${BURN}`, "receipt"]);
  });

  it("resumes from a known burn without burning again", async () => {
    const onBurn = vi.fn();
    await cctpFundSwap(context, {
      swapId: SWAP_ID,
      signer: walletOnOptimism(),
      ownerSigner: owner,
      burnTxHash: BURN,
      amount: 1_000_000n,
      maxFee: 100n,
      onBurn,
    });

    expect(approveAndBurn).not.toHaveBeenCalled();
    expect(onBurn).not.toHaveBeenCalled();
    expect(fetchAttestation).toHaveBeenCalledWith(
      expect.objectContaining({ txHash: BURN, sourceChain: "Optimism" }),
    );
  });
});

describe("Client.fundSwap from a wallet on a CCTP-only chain", () => {
  /** A CCTP-inbound swap from Optimism whose depositor is `depositor`. */
  function cctpSwap(depositor: string, status = "pending") {
    return {
      ...swapResponse({ clientEvmAddress: depositor, gasless: true }),
      status,
      source_amount: "1000000",
      bridge_source_chain: "Optimism",
      bridge_source_token_address: OPTIMISM_USDC,
    } as ReturnType<typeof swapResponse>;
  }

  async function clientWith(
    depositorOf: (sdkAddress: string) => string,
    extra: { cctpBurnTxid?: string; status?: string } = {},
  ) {
    // Stubbed before the client is built: its HTTP client captures `fetch`
    // then. A resumed funding re-reads the swap to check it is still unfunded.
    let response: ReturnType<typeof cctpSwap> | undefined;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = input instanceof Request ? input.url : String(input);
        if (url.endsWith(`/swap/${SWAP_ID}`) && response) {
          return Response.json(response, { status: 200 });
        }
        throw new Error(`unexpected request ${url}`);
      }),
    );
    const storage = new InMemorySwapStorage();
    const client = await Client.builder()
      .withSwapStorage(storage)
      .withAa(AA)
      .build();
    response = cctpSwap(depositorOf(client.getEvmAddress()), extra.status);
    await storage.store(
      storedSwap({ response, cctpBurnTxid: extra.cctpBurnTxid }),
    );
    return { client, storage };
  }

  it("settles a gasless swap with the SDK's own key, not the wallet", async () => {
    const { client } = await clientWith((sdk) => sdk);
    const wallet = walletOnOptimism();

    await client.fundSwap(SWAP_ID, wallet, { maxFee: 100n });

    const owner = submitCctpInboundUserOp.mock.calls[0][1].signer as EvmSigner;
    expect(owner.address.toLowerCase()).toBe(
      client.getEvmAddress().toLowerCase(),
    );
    expect(owner.signAuthorization).toBeTypeOf("function");
    // The wallet still burns, and the mint goes to the SDK key's account.
    expect(approveAndBurn).toHaveBeenCalledWith(
      expect.objectContaining({
        signer: wallet,
        smartAccountAddress: client.getEvmAddress(),
      }),
    );
  });

  it("keeps the wallet as owner for a swap it is the depositor of", async () => {
    const { client } = await clientWith(() => WALLET);
    const wallet = walletOnOptimism();

    await client.fundSwap(SWAP_ID, wallet, { maxFee: 100n });

    expect(submitCctpInboundUserOp.mock.calls[0][1].signer).toBe(wallet);
  });

  it("refuses a swap whose depositor is neither the SDK key nor the wallet", async () => {
    const { client } = await clientWith(() => `0x${"99".repeat(20)}`);

    await expect(
      client.fundSwap(SWAP_ID, walletOnOptimism(), { maxFee: 100n }),
    ).rejects.toThrow(/neither this client's key nor the given wallet/);
    expect(approveAndBurn).not.toHaveBeenCalled();
  });

  it("records the burn on the stored swap", async () => {
    const { client, storage } = await clientWith((sdk) => sdk);

    await client.fundSwap(SWAP_ID, walletOnOptimism(), { maxFee: 100n });

    expect((await storage.get(SWAP_ID))?.cctpBurnTxid).toBe(BURN);
  });

  it("resumes from the recorded burn instead of burning again", async () => {
    const { client } = await clientWith((sdk) => sdk, { cctpBurnTxid: BURN });

    await client.fundSwap(SWAP_ID, walletOnOptimism(), { maxFee: 100n });

    expect(approveAndBurn).not.toHaveBeenCalled();
    expect(fetchAttestation).toHaveBeenCalledWith(
      expect.objectContaining({ txHash: BURN }),
    );
  });

  it("does not resume a swap the recorded burn already funded", async () => {
    const { client } = await clientWith((sdk) => sdk, {
      cctpBurnTxid: BURN,
      status: "clientfundingseen",
    });

    await expect(
      client.fundSwap(SWAP_ID, walletOnOptimism(), { maxFee: 100n }),
    ).rejects.toThrow(/already clientfundingseen/);
    expect(submitCctpInboundUserOp).not.toHaveBeenCalled();
  });

  it("keeps the recorded burn when recovery from the seed rebuilds the swap", async () => {
    const burn = BURN;
    let response: ReturnType<typeof cctpSwap> | undefined;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = input instanceof Request ? input.url : String(input);
        if (url.endsWith("/swap/recover") && response) {
          return Response.json(
            {
              swaps: [{ ...response, index: 0 }],
              highest_index: 0,
              next_index: 1,
              scanned_until: 0,
              complete: true,
            },
            { status: 200 },
          );
        }
        throw new Error(`unexpected request ${url}`);
      }),
    );
    const storage = new InMemorySwapStorage();
    const client = await Client.builder()
      .withSignerStorage(new InMemoryWalletStorage())
      .withSwapStorage(storage)
      .build();
    response = cctpSwap(client.getEvmAddress());
    await storage.store(storedSwap({ response, cctpBurnTxid: burn }));

    await client.recoverSwaps();

    expect((await storage.get(SWAP_ID))?.cctpBurnTxid).toBe(burn);
  });

  it("recovers from the recorded burn with the SDK's key", async () => {
    const { client } = await clientWith((sdk) => sdk, { cctpBurnTxid: BURN });
    recoverCctpInbound.mockResolvedValue({ sweepUserOpHash: USER_OP });
    const recipient = `0x${"77".repeat(20)}`;

    await client.recoverCctpInboundSwap(SWAP_ID, recipient);

    const [, params] = recoverCctpInbound.mock.calls[0];
    expect(params).toMatchObject({
      burnTxHash: BURN,
      sourceChain: "Optimism",
      recipient,
    });
    expect(params.signer.address.toLowerCase()).toBe(
      client.getEvmAddress().toLowerCase(),
    );
  });
});
