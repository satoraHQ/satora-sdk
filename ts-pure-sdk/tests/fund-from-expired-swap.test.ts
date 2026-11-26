import { afterEach, describe, expect, it, vi } from "vitest";
import { Client } from "../src/client.js";
import { deriveEvmAddress } from "../src/evm/signing.js";
import { bytesToHex } from "../src/signer/index.js";
import {
  InMemorySwapStorage,
  InMemoryWalletStorage,
} from "../src/storage/index.js";
import type { StoredSwap } from "../src/storage/types.js";
import { storedSwap, swapResponse, USDC } from "./evm-funding-fixture.js";

const mnemonic =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

async function setup() {
  const storage = new InMemorySwapStorage();
  const bodies: Record<string, unknown>[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      if (new URL(req.url).pathname !== "/swap/evm/bitcoin")
        return new Response("", { status: 404 });
      const body = await req.json();
      bodies.push(body);
      return Response.json({
        ...swapResponse({ gasless: true }),
        id: body.hash_lock,
        status: "pending",
        client_evm_address: body.user_address,
        target_btc_address: body.target_address,
      });
    }),
  );
  const client = await Client.builder()
    .withBaseUrl("https://api.test")
    .withMnemonic(mnemonic)
    .withSwapStorage(storage)
    .build();
  // The expired payment's own per-swap deposit account.
  const params = client.deriveSwapParamsAtIndex(5);
  const depositor = deriveEvmAddress(params.secretKey);
  const old = storedSwap({
    response: swapResponse({ gasless: true, clientEvmAddress: depositor }),
  });
  await storage.store({
    ...old,
    keyIndex: 5,
    secretKey: bytesToHex(params.secretKey),
    // Expired before any funding was sent.
    evmFundTxid: undefined,
    evmFundingUserOpHash: undefined,
  });
  const serverSwap = (overrides: Record<string, unknown> = {}) =>
    vi.spyOn(client, "getSwap").mockResolvedValue({
      ...swapResponse({ gasless: true, clientEvmAddress: depositor }),
      status: "expired",
      ...overrides,
    } as unknown as Awaited<ReturnType<Client["getSwap"]>>);
  const create = () =>
    client.createEvmToBitcoinSwap({
      gasless: true,
      fundFromSwapId: old.swapId,
      evmChainId: 137,
      tokenAddress: USDC,
      userAddress: "",
      targetAddress: "recovery",
      sourceAmount: 100n,
    });
  return { client, storage, bodies, depositor, serverSwap, create, old };
}

describe("retrying an expired payment from its deposit", () => {
  it("creates the new swap on the expired swap's deposit account", async () => {
    const { client, bodies, depositor, serverSwap, create } = await setup();
    serverSwap();
    const created = await create();
    expect(bodies[0].user_address).toBe(depositor);
    // Funding and refunds of the new swap sign with the reused key.
    const key = await client.getSwapDepositorKey(created.response.id);
    expect(key.address).toBe(depositor);
    expect(deriveEvmAddress(key.privateKey)).toBe(depositor);
    const stored = (await client.getStoredSwap(
      created.response.id,
    )) as StoredSwap;
    expect(stored.evmDepositorKeyIndex).toBe(5);
    expect(stored.evmDepositorSecretKey).toBeDefined();
  });

  it("reaches the same account through generic creation", async () => {
    const { client, bodies, depositor, serverSwap, old } = await setup();
    serverSwap();
    await client.createSwap({
      source: { chain: "137", tokenId: USDC },
      target: { chain: "Bitcoin", tokenId: "btc" },
      sourceAmount: 100n,
      targetAddress: "recovery",
      gasless: true,
      fundFromSwapId: old.swapId,
    });
    expect(bodies[0].user_address).toBe(depositor);
  });

  it("refuses a second retry of the same expired swap", async () => {
    const { storage, bodies, serverSwap, create, old } = await setup();
    serverSwap();
    const first = await create();
    expect((await storage.get(old.swapId))?.supersededBy).toBe(
      first.response.id,
    );
    await expect(create()).rejects.toThrow("already retried");
    expect(bodies).toHaveLength(1);
  });

  it("refuses a swap with a funding attempt on record", async () => {
    const { storage, bodies, serverSwap, create, old } = await setup();
    serverSwap();
    const record = (await storage.get(old.swapId)) as StoredSwap;
    await storage.store({
      ...record,
      evmFundingUserOpHash: `0x${"0e".repeat(32)}`,
    });
    await expect(create()).rejects.toThrow("funding attempt");
    expect(bodies).toHaveLength(0);
  });

  it("refuses fundFromSwapId outside EVM to Bitcoin", async () => {
    const { client, bodies, old } = await setup();
    await expect(
      client.createSwap({
        source: { chain: "137", tokenId: USDC },
        target: { chain: "Arkade", tokenId: "btc" },
        sourceAmount: 100n,
        targetAddress: "ark1recovery",
        gasless: true,
        fundFromSwapId: old.swapId,
      }),
    ).rejects.toThrow("EVM to Bitcoin only");
    expect(bodies).toHaveLength(0);
  });

  it.each([
    ["a swap still awaiting funding", { status: "pending" }, "not expired"],
    [
      "a funded swap",
      { status: "expired", evm_fund_txid: "0xabc" },
      "not expired",
    ],
    [
      "another token",
      {
        status: "expired",
        source_token: { token_id: `0x${"22".repeat(20)}`, chain: "137" },
      },
      "same token and chain",
    ],
    [
      "a non-gasless swap",
      { status: "expired", gasless: false },
      "same token and chain",
    ],
  ])("refuses %s", async (_label, overrides, reason) => {
    const { bodies, serverSwap, create } = await setup();
    serverSwap(overrides);
    await expect(create()).rejects.toThrow(reason);
    expect(bodies).toHaveLength(0);
  });
});

it("links a retry to the expired swap's account during seed recovery", async () => {
  let responses: unknown[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      Response.json({
        swaps: responses,
        highest_index: 1,
        next_index: 2,
        scanned_until: 1,
        complete: true,
      }),
    ),
  );
  const client = await Client.builder()
    .withBaseUrl("https://api.test")
    .withMnemonic(mnemonic)
    .withSwapStorage(new InMemorySwapStorage())
    .withSignerStorage(new InMemoryWalletStorage())
    .build();
  const expired = client.deriveSwapParamsAtIndex(0);
  const retry = client.deriveSwapParamsAtIndex(1);
  const depositor = deriveEvmAddress(expired.secretKey);
  responses = [
    {
      ...swapResponse({ clientEvmAddress: depositor, gasless: true }),
      status: "expired",
      index: 0,
      evm_hash_lock: bytesToHex(expired.preimageHash),
    },
    {
      ...swapResponse({ clientEvmAddress: depositor, gasless: true }),
      id: "3d41d067-cc82-4ddc-9f4e-e49a82b65a33",
      index: 1,
      evm_hash_lock: bytesToHex(retry.preimageHash),
    },
  ];
  await client.recoverSwaps();
  const key = await client.getSwapDepositorKey(
    "3d41d067-cc82-4ddc-9f4e-e49a82b65a33",
  );
  expect(key.address.toLowerCase()).toBe(depositor.toLowerCase());
  expect(key.keyIndex).toBe(0);
});
