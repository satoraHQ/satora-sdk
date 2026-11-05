import { afterEach, describe, expect, it, vi } from "vitest";
import { invoiceAmountMsat } from "../src/bolt11.js";
import { buildCctpInboundBatch } from "../src/cctp-inbound/userOp.js";
import { computeCoordinatorCallsHash } from "../src/evm/coordinator.js";
import { Client, InMemorySwapStorage } from "../src/index.js";
import { INVOICE, INVOICE_SATS, PAYMENT_HASH } from "./bolt11-fixture.js";
import {
  COORDINATOR,
  fundingTerms,
  HASH_LOCK,
  SERVER,
  SIGNER_ADDRESS,
  SWAP_ID,
  serverAnswering,
  signerWith,
  storedSwap,
  swapResponse,
  TIMELOCK,
  USDC,
  WBTC,
} from "./evm-funding-fixture.js";

// A hash whose preimage the server could know: not this swap's.
const FOREIGN = `0x${"f0".repeat(32)}`;

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("invoiceAmountMsat", () => {
  it("reads the amount off the human-readable part", () => {
    expect(invoiceAmountMsat(INVOICE)).toBe(BigInt(INVOICE_SATS) * 1000n);
  });
});

describe("createEvmToLightningSwap fixes the payment hash", () => {
  /** The server answering the create with `answer`. */
  async function creating(answer: Record<string, unknown>) {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : String(input);
      if (url.endsWith("/swap/evm/lightning")) {
        return Response.json({
          ...swapResponse({ direction: "evm_to_lightning" }),
          ...answer,
        });
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const storage = new InMemorySwapStorage();
    const client = await Client.builder().withSwapStorage(storage).build();
    return { client, storage };
  }
  const common = {
    evmChainId: 137,
    tokenAddress: USDC,
    userAddress: SIGNER_ADDRESS,
  };

  it("records the invoice's own payment hash", async () => {
    const { client, storage } = await creating({ hash_lock: PAYMENT_HASH });

    await client.createEvmToLightningSwap({
      ...common,
      lightningInvoice: INVOICE,
    });

    expect((await storage.get(SWAP_ID))?.lightningPaymentHash).toEqual({
      hash: PAYMENT_HASH,
      source: "invoice",
    });
    // The spender every later funding must name is the one named now.
    expect((await storage.get(SWAP_ID))?.evmFundingTerms).toMatchObject({
      coordinator: COORDINATOR,
      claimAddress: SERVER,
      timelock: TIMELOCK,
      sourceToken: USDC,
    });
  });

  it("refuses a swap created under another hash than the invoice's", async () => {
    const { client, storage } = await creating({ hash_lock: FOREIGN });

    await expect(
      client.createEvmToLightningSwap({ ...common, lightningInvoice: INVOICE }),
    ).rejects.toThrow("not the invoice's payment hash");
    expect(await storage.get(SWAP_ID)).toBeNull();
  });

  it("binds a Lightning address swap to the invoice the server resolved", async () => {
    const { client, storage } = await creating({
      hash_lock: PAYMENT_HASH,
      client_lightning_invoice: INVOICE,
      target_amount: String(INVOICE_SATS),
    });

    await client.createEvmToLightningSwap({
      ...common,
      lightningAddress: "user@example.com",
      targetAmountSats: INVOICE_SATS,
    });

    expect((await storage.get(SWAP_ID))?.lightningPaymentHash).toEqual({
      hash: PAYMENT_HASH,
      source: "server-invoice",
    });
  });

  it.each([
    [
      "a hash lock other than its invoice's",
      {
        hash_lock: FOREIGN,
        client_lightning_invoice: INVOICE,
        target_amount: String(INVOICE_SATS),
      },
      {},
      "not the payment hash",
    ],
    [
      "an invoice for another amount than the quoted payout",
      {
        hash_lock: PAYMENT_HASH,
        client_lightning_invoice: INVOICE,
        target_amount: String(INVOICE_SATS + 1),
      },
      {},
      `not the ${INVOICE_SATS + 1} sats payout`,
    ],
    [
      "an invoice for another amount than the one asked",
      {
        hash_lock: PAYMENT_HASH,
        client_lightning_invoice: INVOICE,
        target_amount: String(INVOICE_SATS),
      },
      { targetAmountSats: 1000 },
      "not the 1000 sats payout",
    ],
    [
      "no invoice at all",
      { hash_lock: PAYMENT_HASH, client_lightning_invoice: undefined },
      {},
      "returned no invoice",
    ],
  ])("refuses a Lightning address swap with %s", async (_, answer, asked: {
    targetAmountSats?: number;
  }, reason) => {
    const { client, storage } = await creating(answer);

    await expect(
      client.createEvmToLightningSwap({
        ...common,
        lightningAddress: "user@example.com",
        ...(asked.targetAmountSats === undefined
          ? { sourceAmount: 1_000_000n }
          : { targetAmountSats: asked.targetAmountSats }),
      } as Parameters<Client["createEvmToLightningSwap"]>[0]),
    ).rejects.toThrow(reason);
    expect(await storage.get(SWAP_ID)).toBeNull();
  });
});

describe("every funding path refuses a hash lock the client did not fix", () => {
  const permit2 = (preimageHash: string) => ({
    coordinator_address: COORDINATOR,
    permit2_address: "0x000000000022d473030f116ddee9f6b43ac78ba3",
    source_token_address: USDC,
    source_amount: "15658166",
    lock_token_address: WBTC,
    preimage_hash: preimageHash,
    claim_address: SERVER,
    timelock: TIMELOCK,
    calls: [],
    calls_hash: computeCoordinatorCallsHash([]),
  });
  const MNEMONIC =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
  const requested = (fetchMock: ReturnType<typeof vi.fn>, path: string) =>
    fetchMock.mock.calls.some(([input]) =>
      (input instanceof Request ? input.url : String(input)).includes(path),
    );

  async function clientHolding(
    stored = storedSwap({ evmFundTxid: undefined }),
  ) {
    const storage = new InMemorySwapStorage();
    await storage.store(stored);
    return Client.builder()
      .withSwapStorage(storage)
      .withMnemonic(MNEMONIC)
      .build();
  }

  it("fundSwapGasless: refuses before signing and never calls the relay", async () => {
    const fetchMock = serverAnswering({
      swap: swapResponse(),
      permit2: permit2(FOREIGN),
      gasless: { id: SWAP_ID, status: "clientfunded", tx_hash: "0x01" },
    });
    const client = await clientHolding();

    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      `not the swap's ${HASH_LOCK}`,
    );
    expect(requested(fetchMock, "fund-gasless")).toBe(false);
  });

  it("fundSwapGasless: refuses an EVM -> Lightning swap with no payment hash on record", async () => {
    const fetchMock = serverAnswering({
      swap: swapResponse({ direction: "evm_to_lightning" }),
      permit2: permit2(HASH_LOCK),
      gasless: { id: SWAP_ID, status: "clientfunded", tx_hash: "0x01" },
    });
    const client = await clientHolding(
      storedSwap({
        direction: "evm_to_lightning",
        evmFundTxid: undefined,
        lightningPaymentHash: undefined,
      }),
    );

    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      "no payment hash recorded",
    );
    expect(requested(fetchMock, "fund-gasless")).toBe(false);
  });

  it("getCoordinatorFundingCallDataPermit2: refuses before signing", async () => {
    serverAnswering({ swap: swapResponse(), permit2: permit2(FOREIGN) });
    const client = await clientHolding();

    await expect(
      client.getCoordinatorFundingCallDataPermit2(SWAP_ID, 137),
    ).rejects.toThrow("not the swap's");
  });

  it("getPermit2FundingParamsUnsigned: refuses to hand out the params", async () => {
    serverAnswering({ swap: swapResponse(), permit2: permit2(FOREIGN) });
    const client = await clientHolding();

    await expect(
      client.getPermit2FundingParamsUnsigned(SWAP_ID, 137, SIGNER_ADDRESS),
    ).rejects.toThrow("not the swap's");
  });

  it("fundSwap (Permit2): the wallet signs and sends nothing", async () => {
    serverAnswering({ swap: swapResponse(), permit2: permit2(FOREIGN) });
    const client = await clientHolding();
    const signTypedData = vi.fn(async () => `0x${"11".repeat(64)}1b`);
    const sendTransaction = vi.fn(async () => `0x${"aa".repeat(32)}`);

    await expect(
      client.fundSwap(SWAP_ID, signerWith({ signTypedData, sendTransaction })),
    ).rejects.toThrow("not the swap's");
    expect(signTypedData).not.toHaveBeenCalled();
  });

  it("fundSwap (CCTP-inbound): refuses before anything is burned", async () => {
    const response = {
      ...swapResponse({ direction: "evm_to_lightning" }),
      hash_lock: FOREIGN,
      evm_chain_id: 42161,
      bridge_source_chain: "optimism",
      bridge_source_token_address: "0x0b2c639c533813f4aa9d7837caf62653d097ff85",
    } as ReturnType<typeof swapResponse>;
    serverAnswering({ swap: response });
    const client = await clientHolding(
      storedSwap({ direction: "evm_to_lightning", response }),
    );
    const sendTransaction = vi.fn(async () => `0x${"aa".repeat(32)}`);

    await expect(
      client.fundSwap(SWAP_ID, signerWith({ chainId: 10, sendTransaction })),
    ).rejects.toThrow("not this swap's");
    expect(sendTransaction).not.toHaveBeenCalled();
  });

  it("the CCTP / Solana / continuation settlement batch refuses before signing", async () => {
    const signTypedData = vi.fn(async () => `0x${"11".repeat(65)}`);

    await expect(
      buildCctpInboundBatch({
        server: {
          ...permit2(FOREIGN),
          htlc_kind: "erc20",
          htlc_version: 7,
          aa: { entry_point: "0x", delegation_target: "0x" },
        },
        expected: {
          hashLock: HASH_LOCK,
          terms: fundingTerms({ chainId: 42161 }),
        },
        smartAccountAddress: SIGNER_ADDRESS as `0x${string}`,
        signTypedData: signTypedData as never,
        cctpMessage: "0x",
        cctpAttestation: "0x",
        chainId: 42161,
      }),
    ).rejects.toThrow("not the swap's");
    expect(signTypedData).not.toHaveBeenCalled();
  });
});
