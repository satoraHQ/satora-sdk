import { afterEach, describe, expect, it, vi } from "vitest";
import { createArkadeToLightningSwap } from "../src/create/arkade-to-lightning.js";
import { createBitcoinToLightningSwap } from "../src/create/bitcoin-to-lightning.js";
import { createLightningToArkadeSwap } from "../src/create/lightning-to-arkade.js";
import { createLightningToBitcoinSwap } from "../src/create/lightning-to-bitcoin.js";
import { createLightningToEvmSwap } from "../src/create/lightning-to-evm.js";
import type { CreateSwapContext } from "../src/create/types.js";
import {
  Client,
  InMemorySwapStorage,
  InMemoryWalletStorage,
  type StoredSwap,
  SwapVerificationError,
} from "../src/index.js";
import type { SwapParams } from "../src/signer/index.js";
import vectors from "./fixtures/lightning-invoices.js";

const { invoices, swaps } = vectors;
const L2A = swaps.lightning_to_arkade.response;
const A2L = swaps.arkade_to_lightning.response;
const A2L_INVOICE = A2L.client_lightning_invoice;
const B2L = swaps.bitcoin_to_lightning.response;
const L2B = swaps.lightning_to_bitcoin.response;
const BASE_URL = "http://swap.test";

async function paramsAt(index: number): Promise<SwapParams> {
  const client = await Client.builder().withMnemonic(vectors.mnemonic).build();
  return client.deriveSwapParamsAtIndex(index);
}

/** A create context whose server answers every POST with `response`. */
function contextAnswering(params: SwapParams, response: unknown) {
  const post = vi.fn(async () => ({
    data: response,
    error: undefined,
    response: new Response(null, { status: 200 }),
  }));
  const derive = vi.fn(async () => params);
  const stored: unknown[] = [];
  const ctx = {
    apiClient: { POST: post } as unknown as CreateSwapContext["apiClient"],
    baseUrl: BASE_URL,
    deriveSwapParams: derive,
    evmAddress: "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
    storeSwap: async (...args: unknown[]) => {
      stored.push(args);
    },
  } satisfies CreateSwapContext;
  return { ctx, post, derive, stored };
}

afterEach(() => vi.unstubAllGlobals());

async function refusal(
  promise: Promise<unknown>,
): Promise<SwapVerificationError> {
  const error = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(SwapVerificationError);
  return error as SwapVerificationError;
}

/**
 * A client whose server answers `GET /swap/{id}` and `POST /swap/recover`
 * from `routes`, and creation with the regtest daemon's real responses.
 */
async function clientWith(routes: {
  get?: Record<string, unknown>;
  recover?: unknown[];
}) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = new Request(input, init);
      const path = new URL(request.url).pathname;
      const answer = (body: unknown) => Response.json(body, { status: 200 });
      if (path === "/swap/lightning/arkade") return answer(L2A);
      if (path === "/swap/arkade/lightning") return answer(A2L);
      if (path === "/swap/recover") {
        return answer({
          swaps: routes.recover ?? [],
          highest_index: 1,
          next_index: 2,
          scanned_until: 1,
          complete: true,
        });
      }
      const id = path.match(/^\/swap\/([0-9a-f-]+)$/)?.[1];
      if (id && routes.get?.[id]) return answer(routes.get[id]);
      throw new Error(`unexpected request ${request.method} ${path}`);
    }),
  );
  const storage = new InMemorySwapStorage();
  const warnings: unknown[] = [];
  const client = await Client.builder()
    .withBaseUrl(BASE_URL)
    .withMnemonic(vectors.mnemonic)
    .withSignerStorage(new InMemoryWalletStorage())
    .withSwapStorage(storage)
    .withLogger({ warn: (record) => warnings.push(record) })
    .withLogLevel("warn")
    .build();
  return { client, storage, warnings };
}

describe("Lightning to X creation checks the hold invoice's payment hash", () => {
  const arkadeOptions = {
    targetAddress: L2A.target_arkade_address,
    sourceAmountSats: 20000,
  };
  const evmOptions = {
    targetAddress: "0x000000000000000000000000000000000000dEaD",
    tokenAddress: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
    evmChainId: 42161,
    sourceAmount: 1000,
  };

  it("stores the regtest daemon's hold invoice, which pays the swap's hash lock", async () => {
    const { ctx, stored } = contextAnswering(await paramsAt(1), L2A);
    const result = await createLightningToArkadeSwap(arkadeOptions, ctx);
    expect(result.response.bolt11_invoice).toBe(L2A.bolt11_invoice);
    expect(stored).toHaveLength(1);
  });

  it("refuses a Lightning to Arkade invoice on another hash, before storing or retrying", async () => {
    const { ctx, post, stored } = contextAnswering(await paramsAt(1), {
      ...L2A,
      bolt11_invoice: invoices.lnd_hold_22.bolt11,
    });
    const error = await refusal(
      createLightningToArkadeSwap(arkadeOptions, ctx),
    );
    expect(error.field).toBe("bolt11_invoice");
    expect(error.received).toBe(invoices.lnd_hold_22.payment_hash);
    expect(error.expected).toBe(swaps.lightning_to_arkade.invoice_payment_hash);
    expect(stored).toHaveLength(0);
    expect(post).toHaveBeenCalledTimes(1);
  });

  it("stores a Lightning to EVM invoice on the swap's hash lock", async () => {
    const { ctx, stored } = contextAnswering(await paramsAt(7), {
      id: "11111111-2222-3333-4444-555555555555",
      bolt11_invoice: invoices.lnd_hold_index_7.bolt11,
    });
    await createLightningToEvmSwap(evmOptions, ctx);
    expect(stored).toHaveLength(1);
  });

  it("stores the regtest daemon's Lightning to Bitcoin hold invoice", async () => {
    const { ctx, stored } = contextAnswering(await paramsAt(3), L2B);
    await createLightningToBitcoinSwap(
      { targetAddress: L2B.target_btc_address, sourceAmountSats: 20000 },
      ctx,
    );
    expect(stored).toHaveLength(1);
  });

  it("refuses a Lightning to Bitcoin invoice on another hash", async () => {
    const { ctx, post, stored } = contextAnswering(await paramsAt(3), {
      ...L2B,
      bolt11_invoice: invoices.lnd_hold_22.bolt11,
    });
    const error = await refusal(
      createLightningToBitcoinSwap(
        { targetAddress: L2B.target_btc_address, sourceAmountSats: 20000 },
        ctx,
      ),
    );
    expect(error.field).toBe("bolt11_invoice");
    expect(error.expected).toBe(
      swaps.lightning_to_bitcoin.invoice_payment_hash,
    );
    expect(stored).toHaveLength(0);
    expect(post).toHaveBeenCalledTimes(1);
  });

  it("refuses a Lightning to EVM invoice on another hash", async () => {
    const { ctx, post, stored } = contextAnswering(await paramsAt(7), {
      id: "11111111-2222-3333-4444-555555555555",
      bolt11_invoice: invoices.lnd_hold_index_0.bolt11,
    });
    const error = await refusal(createLightningToEvmSwap(evmOptions, ctx));
    expect(error.field).toBe("bolt11_invoice");
    expect(stored).toHaveLength(0);
    expect(post).toHaveBeenCalledTimes(1);
  });
});

describe("X to Lightning creation checks the hash lock against the invoice", () => {
  type Destination =
    | { lightningInvoice: string }
    | { lightningAddress: string };
  const withAmount = (destination: Destination) =>
    "lightningInvoice" in destination
      ? destination
      : { ...destination, targetAmountSats: 20000 };
  const directions = [
    {
      name: "Arkade to Lightning",
      response: A2L as typeof A2L | typeof B2L,
      invoice: A2L_INVOICE,
      index: 1,
      create: (destination: Destination, ctx: CreateSwapContext) =>
        createArkadeToLightningSwap(withAmount(destination), ctx),
    },
    {
      name: "Bitcoin to Lightning",
      response: B2L,
      invoice: B2L.client_lightning_invoice,
      index: 2,
      create: (destination: Destination, ctx: CreateSwapContext) =>
        createBitcoinToLightningSwap(withAmount(destination), ctx),
    },
  ];

  describe.each(directions)("$name", ({ response, invoice, index, create }) => {
    it("stores a swap locked to the supplied invoice's payment hash", async () => {
      const { ctx, stored } = contextAnswering(await paramsAt(index), response);
      await create({ lightningInvoice: invoice }, ctx);
      expect(stored).toHaveLength(1);
    });

    it("refuses a hash lock that is not the invoice's payment hash", async () => {
      const { ctx, stored } = contextAnswering(await paramsAt(index), {
        ...response,
        hash_lock: `0x${invoices.lnd_hold_22.payment_hash}`,
      });
      const error = await refusal(create({ lightningInvoice: invoice }, ctx));
      expect(error.field).toBe("hash_lock");
      expect(stored).toHaveLength(0);
    });

    it("refuses another invoice in place of the supplied one, even with a matching lock", async () => {
      const { ctx, stored } = contextAnswering(await paramsAt(index), {
        ...response,
        client_lightning_invoice: invoices.lnd_hold_33.bolt11,
        hash_lock: `0x${invoices.lnd_hold_33.payment_hash}`,
      });
      const error = await refusal(create({ lightningInvoice: invoice }, ctx));
      expect(error.field).toBe("client_lightning_invoice");
      expect(error.received).toBe(invoices.lnd_hold_33.payment_hash);
      expect(stored).toHaveLength(0);
    });

    it("checks a server-resolved invoice for a Lightning address against the lock", async () => {
      const honest = contextAnswering(await paramsAt(index), {
        ...response,
        hash_lock: `0x${response.hash_lock.slice(2).toUpperCase()}`,
      });
      await create({ lightningAddress: "payee@lnurl.test" }, honest.ctx);
      expect(honest.stored).toHaveLength(1);

      const lying = contextAnswering(await paramsAt(index), {
        ...response,
        hash_lock: `0x${invoices.lnd_hold_22.payment_hash}`,
      });
      const error = await refusal(
        create({ lightningAddress: "payee@lnurl.test" }, lying.ctx),
      );
      expect(error.field).toBe("hash_lock");
      expect(lying.stored).toHaveLength(0);
    });

    it("refuses a server-resolved invoice for another amount than the payout", async () => {
      const { ctx, stored } = contextAnswering(await paramsAt(index), {
        ...response,
        target_amount: "19000",
      });
      const error = await refusal(
        create({ lightningAddress: "payee@lnurl.test" }, ctx),
      );
      expect(error.field).toBe("client_lightning_invoice");
      expect(error.expected).toBe("19000 sats");
      expect(stored).toHaveLength(0);
    });

    it("refuses a malformed invoice before any request or key derivation", async () => {
      const { ctx, post, derive } = contextAnswering(
        await paramsAt(index),
        response,
      );
      await expect(
        create({ lightningInvoice: "lnbcrt1fixture" }, ctx),
      ).rejects.toThrow(/Not a Lightning invoice/);
      expect(post).not.toHaveBeenCalled();
      expect(derive).not.toHaveBeenCalled();
    });
  });
});

describe("Arkade to Lightning creation rebuilds the VHTLC it asks the user to fund", () => {
  const create = (ctx: CreateSwapContext) =>
    createArkadeToLightningSwap({ lightningInvoice: A2L_INVOICE }, ctx);

  it("refuses an address the server built on another hash", async () => {
    const { ctx, stored } = contextAnswering(await paramsAt(1), {
      ...A2L,
      arkade_vhtlc_address: L2A.arkade_vhtlc_address,
    });
    const error = await refusal(create(ctx));
    expect(error.field).toBe("arkade_vhtlc_address");
    expect(error.expected).toBe(A2L.arkade_vhtlc_address);
    expect(stored).toHaveLength(0);
  });

  it("refuses a VHTLC that refunds to another key", async () => {
    const { ctx, stored } = contextAnswering(await paramsAt(0), A2L);
    const error = await refusal(create(ctx));
    expect(error.field).toBe("arkade_vhtlc_address");
    expect(stored).toHaveLength(0);
  });

  it("refuses a script version this client cannot refund", async () => {
    const { ctx, stored } = contextAnswering(await paramsAt(1), {
      ...A2L,
      arkade_htlc_script_version: 2,
    });
    const error = await refusal(create(ctx));
    expect(error.field).toBe("arkade_htlc_script_version");
    expect(stored).toHaveLength(0);
  });
});

describe("Bitcoin to Lightning creation rebuilds the HTLC it asks the user to fund", () => {
  const create = (ctx: CreateSwapContext) =>
    createBitcoinToLightningSwap(
      { lightningInvoice: B2L.client_lightning_invoice },
      ctx,
    );

  // The Lightning to Bitcoin capture's HTLC is a real address on another hash.
  it("refuses an address the server built on another hash", async () => {
    const { ctx, stored } = contextAnswering(await paramsAt(2), {
      ...B2L,
      btc_htlc_address: L2B.btc_htlc_address,
    });
    const error = await refusal(create(ctx));
    expect(error.field).toBe("btc_htlc_address");
    expect(stored).toHaveLength(0);
  });

  it("refuses an HTLC that refunds to another key", async () => {
    const { ctx, stored } = contextAnswering(await paramsAt(1), B2L);
    const error = await refusal(create(ctx));
    expect(error.field).toBe("btc_htlc_address");
    expect(stored).toHaveLength(0);
  });

  // The refund rebuilds the HTLC from btc_hash_lock, so a wrong one would
  // leave the deposit unrefundable even with the right address.
  it("refuses an HTLC hash that is not the invoice's", async () => {
    const { ctx, stored } = contextAnswering(await paramsAt(2), {
      ...B2L,
      btc_hash_lock: L2B.btc_hash_lock,
    });
    const error = await refusal(create(ctx));
    expect(error.field).toBe("btc_hash_lock");
    expect(stored).toHaveLength(0);
  });

  it("refuses a script version this client cannot refund", async () => {
    const { ctx, stored } = contextAnswering(await paramsAt(2), {
      ...B2L,
      btc_htlc_script_version: 2,
    });
    const error = await refusal(create(ctx));
    expect(error.field).toBe("btc_htlc_script_version");
    expect(stored).toHaveLength(0);
  });
});

describe("a server refresh cannot replace a stored swap's terms", () => {
  async function storedArkadeToLightning(): Promise<{
    client: Client;
    storage: InMemorySwapStorage;
    stored: StoredSwap;
  }> {
    const { client, storage } = await clientWith({});
    await client.setKeyIndex(1);
    await client.createArkadeToLightningSwap({ lightningInvoice: A2L_INVOICE });
    const stored = await storage.get(A2L.id);
    if (!stored) throw new Error("the swap was not stored");
    return { client, storage, stored };
  }

  const refreshed = (changes: Record<string, unknown>) => ({
    ...A2L,
    direction: "arkade_to_lightning",
    status: "clientfunded",
    ...changes,
  });

  it("takes an honest refresh", async () => {
    const { stored } = await storedArkadeToLightning();
    const { client, storage } = await clientWith({
      get: { [A2L.id]: refreshed({}) },
    });
    await storage.store(stored);
    await client.getSwap(A2L.id, { updateStorage: true });
    expect((await storage.get(A2L.id))?.response.status).toBe("clientfunded");
  });

  // Their funding uses what creation recorded, and a rehash moves the lock's
  // hash and invoice, so an EVM-sourced refresh is not compared.
  it("leaves an EVM-sourced swap's refresh to what its creation recorded", async () => {
    const { stored } = await storedArkadeToLightning();
    const E2L_ID = "22222222-3333-4444-5555-666666666666";
    const evmSwap = {
      ...stored,
      swapId: E2L_ID,
      response: {
        id: E2L_ID,
        direction: "evm_to_lightning",
        client_lightning_invoice: invoices.spark_regtest.bolt11,
        hash_lock: `0x${invoices.spark_regtest.payment_hash}`,
      },
    } as unknown as StoredSwap;
    const rehashed = {
      ...evmSwap.response,
      client_lightning_invoice: invoices.lnd_hold_22.bolt11,
      hash_lock: `0x${invoices.lnd_hold_22.payment_hash}`,
    };
    const { client, storage } = await clientWith({
      get: { [E2L_ID]: rehashed },
    });
    await storage.store(evmSwap);

    await client.getSwap(E2L_ID, { updateStorage: true });
    expect((await storage.get(E2L_ID))?.response).toMatchObject(rehashed);
  });

  it.each([
    ["direction", { direction: "evm_to_lightning" }],
    [
      "client_lightning_invoice",
      { client_lightning_invoice: invoices.lnd_hold_22.bolt11 },
    ],
    ["hash_lock", { hash_lock: `0x${invoices.lnd_hold_22.payment_hash}` }],
    [
      "arkade_vhtlc_address",
      {
        arkade_vhtlc_address:
          swaps.lightning_to_arkade.response.arkade_vhtlc_address,
      },
    ],
  ])("keeps the stored terms when a refresh changes %s, and takes its status", async (field, changes) => {
    const { stored } = await storedArkadeToLightning();
    const { client, storage, warnings } = await clientWith({
      get: { [A2L.id]: refreshed(changes) },
    });
    await storage.store(stored);

    const read = await client.getSwap(A2L.id, { updateStorage: true });
    const kept = { ...stored.response, status: "clientfunded" };
    expect(read).toEqual(kept);
    expect((await storage.get(A2L.id))?.response).toEqual(kept);
    expect(warnings).toMatchObject([
      {
        event: "client.get_swap.terms_changed",
        swapId: A2L.id,
        data: { field },
      },
    ]);
  });

  // An older SDK may have stored a response without some field; a refused
  // refresh keeps the server's value for it, unless that is what was refused.
  it("keeps a term the stored copy lacks from a refused refresh", async () => {
    const { stored } = await storedArkadeToLightning();
    const { arkade_vhtlc_address: _, ...withoutAddress } =
      stored.response as typeof stored.response & {
        arkade_vhtlc_address: string;
      };
    const older = { ...stored, response: withoutAddress } as StoredSwap;
    const { client, storage } = await clientWith({
      get: {
        [A2L.id]: refreshed({
          client_lightning_invoice: invoices.lnd_hold_22.bolt11,
        }),
      },
    });
    await storage.store(older);

    const read = await client.getSwap(A2L.id, { updateStorage: true });
    expect(read).toMatchObject({
      arkade_vhtlc_address: A2L.arkade_vhtlc_address,
      client_lightning_invoice: A2L_INVOICE,
      status: "clientfunded",
    });
  });

  it.each([
    ["bolt11_invoice", { bolt11_invoice: invoices.lnd_hold_22.bolt11 }],
    ["hash_lock", { hash_lock: `0x${invoices.lnd_hold_22.payment_hash}` }],
  ])("keeps a Lightning to Arkade swap's stored terms when a refresh changes %s", async (field, changes) => {
    const { client, storage, warnings } = await clientWith({
      get: {
        [L2A.id]: {
          ...L2A,
          direction: "lightning_to_arkade",
          status: "clientfunded",
          ...changes,
        },
      },
    });
    await client.setKeyIndex(1);
    await client.createLightningToArkadeSwap({
      targetAddress: L2A.target_arkade_address,
      sourceAmountSats: 20000,
    });
    const before = await storage.get(L2A.id);

    const read = await client.getSwap(L2A.id, { updateStorage: true });
    expect(read).toEqual({ ...before?.response, status: "clientfunded" });
    expect(warnings).toMatchObject([
      { event: "client.get_swap.terms_changed", data: { field } },
    ]);
  });

  // A swap stored before creation checked anything can hold the server's
  // terms unchecked. Its refreshes all fail the checks, and must still bring
  // its status, or the swap could never be followed through the SDK again.
  it("keeps reading the status of unchanged terms that never bound, as an older SDK stored them", async () => {
    const { stored } = await storedArkadeToLightning();
    const badLock = `0x${invoices.lnd_hold_22.payment_hash}`;
    const legacyArkade = {
      ...stored,
      response: { ...stored.response, hash_lock: badLock },
    } as StoredSwap;
    const legacyLightning = {
      ...legacyArkade,
      swapId: L2A.id,
      preimageHash: swaps.lightning_to_arkade.invoice_payment_hash,
      response: {
        ...L2A,
        direction: "lightning_to_arkade",
        bolt11_invoice: invoices.lnd_hold_22.bolt11,
      },
    } as StoredSwap;
    const { client, storage, warnings } = await clientWith({
      get: {
        [A2L.id]: refreshed({ hash_lock: badLock }),
        [L2A.id]: { ...legacyLightning.response, status: "serverfunded" },
      },
    });
    await storage.store(legacyArkade);
    await storage.store(legacyLightning);

    expect((await client.getSwap(A2L.id, { updateStorage: true })).status).toBe(
      "clientfunded",
    );
    expect((await client.getSwap(L2A.id, { updateStorage: true })).status).toBe(
      "serverfunded",
    );
    expect((await storage.get(L2A.id))?.response.status).toBe("serverfunded");
    expect(warnings).toMatchObject([
      { swapId: A2L.id, data: { field: "hash_lock" } },
      { swapId: L2A.id, data: { field: "bolt11_invoice" } },
    ]);
  });

  it("keeps the stored copy when recovery returns changed terms", async () => {
    const { stored } = await storedArkadeToLightning();
    const { client, storage, warnings } = await clientWith({
      recover: [
        {
          ...refreshed({
            client_lightning_invoice: invoices.lnd_hold_22.bolt11,
          }),
          index: 1,
        },
      ],
    });
    await storage.store(stored);

    const result = await client.recoverSwaps();
    expect(result.swaps).toEqual([stored]);
    expect(await storage.get(A2L.id)).toEqual(stored);
    expect(warnings).toMatchObject([
      {
        event: "client.recover.terms_changed",
        swapId: A2L.id,
        data: { field: "client_lightning_invoice" },
      },
    ]);
  });

  // This VHTLC refunds to the stored key; keys from another index could not
  // refund it.
  it("keeps the stored keys when recovery names another index for unchanged terms", async () => {
    const { stored } = await storedArkadeToLightning();
    const { client, storage, warnings } = await clientWith({
      recover: [{ ...refreshed({}), index: stored.keyIndex + 1 }],
    });
    await storage.store(stored);

    const result = await client.recoverSwaps();
    expect(result.swaps).toEqual([stored]);
    expect(await storage.get(A2L.id)).toEqual(stored);
    expect(warnings).toMatchObject([
      {
        event: "client.recover.terms_changed",
        swapId: A2L.id,
        data: { field: "index" },
      },
    ]);
  });

  it("takes an honest recovery at the stored index", async () => {
    const { stored } = await storedArkadeToLightning();
    const { client, storage, warnings } = await clientWith({
      recover: [{ ...refreshed({}), index: stored.keyIndex }],
    });
    await storage.store(stored);

    await client.recoverSwaps();
    const recovered = await storage.get(A2L.id);
    expect(recovered?.response.status).toBe("clientfunded");
    expect(recovered?.preimageHash).toBe(stored.preimageHash);
    expect(warnings).toEqual([]);
  });
});

describe("recovery checks a swap it stores for the first time", () => {
  const recovered = <T extends { id: string }>(
    response: T,
    direction: string,
    index = 1,
  ) => ({
    ...response,
    direction,
    index,
  });
  const L2E = {
    id: "11111111-2222-3333-4444-555555555555",
    bolt11_invoice: invoices.lnd_hold_index_7.bolt11,
  };
  const E2L = {
    id: "22222222-3333-4444-5555-666666666666",
    client_lightning_invoice: invoices.spark_regtest.bolt11,
    hash_lock: `0x${invoices.spark_regtest.payment_hash}`,
  };
  const otherLock = `0x${invoices.lnd_hold_22.payment_hash}`;

  it("stores swaps whose terms bind to the keys at the server's index", async () => {
    const { client, storage, warnings } = await clientWith({
      recover: [
        recovered(L2A, "lightning_to_arkade"),
        recovered(L2E, "lightning_to_evm", 7),
        recovered(A2L, "arkade_to_lightning"),
        recovered(E2L, "evm_to_lightning"),
        recovered(B2L, "bitcoin_to_lightning", 2),
        recovered(L2B, "lightning_to_bitcoin", 3),
      ],
    });

    const result = await client.recoverSwaps();
    expect(result.swaps.map((swap) => swap.swapId)).toEqual([
      L2A.id,
      L2E.id,
      A2L.id,
      E2L.id,
      B2L.id,
      L2B.id,
    ]);
    expect((await storage.get(L2E.id))?.keyIndex).toBe(7);
    expect(warnings).toEqual([]);
  });

  it.each([
    [
      "a Lightning to Arkade invoice on another hash",
      recovered(
        { ...L2A, bolt11_invoice: invoices.lnd_hold_22.bolt11 },
        "lightning_to_arkade",
      ),
      "bolt11_invoice",
    ],
    [
      "a Lightning to EVM invoice that does not pay the secret at the server's index",
      recovered(L2E, "lightning_to_evm", 0),
      "bolt11_invoice",
    ],
    [
      "an Arkade to Lightning hash lock that is not its invoice's payment hash",
      recovered({ ...A2L, hash_lock: otherLock }, "arkade_to_lightning"),
      "hash_lock",
    ],
    // A fresh device cannot know which Lightning address the swap paid, so
    // the invoice must at least be for the payout the server quoted.
    [
      "an Arkade to Lightning invoice for another amount than the payout",
      recovered({ ...A2L, target_amount: "1000" }, "arkade_to_lightning"),
      "client_lightning_invoice",
    ],
    [
      "an Arkade to Lightning VHTLC that refunds to another key than the one at the server's index",
      recovered(A2L, "arkade_to_lightning", 0),
      "arkade_vhtlc_address",
    ],
    [
      "an Arkade to Lightning VHTLC address built on another hash",
      recovered(
        { ...A2L, arkade_vhtlc_address: L2A.arkade_vhtlc_address },
        "arkade_to_lightning",
      ),
      "arkade_vhtlc_address",
    ],
    [
      "an Arkade to Lightning swap on a script version this client cannot refund",
      recovered(
        { ...A2L, arkade_htlc_script_version: 0 },
        "arkade_to_lightning",
      ),
      "arkade_htlc_script_version",
    ],
    [
      "a Bitcoin to Lightning hash lock that is not its invoice's payment hash",
      recovered({ ...B2L, hash_lock: otherLock }, "bitcoin_to_lightning", 2),
      "hash_lock",
    ],
    [
      "a Bitcoin to Lightning invoice for another amount than the payout",
      recovered({ ...B2L, target_amount: "1000" }, "bitcoin_to_lightning", 2),
      "client_lightning_invoice",
    ],
    [
      "a Bitcoin to Lightning HTLC that refunds to another key than the one at the server's index",
      recovered(B2L, "bitcoin_to_lightning", 1),
      "btc_htlc_address",
    ],
    [
      "a Lightning to Bitcoin invoice that does not pay the secret at the server's index",
      recovered(L2B, "lightning_to_bitcoin", 2),
      "bolt11_invoice",
    ],
  ])("refuses %s and stores nothing", async (_, swap, field) => {
    const { client, storage, warnings } = await clientWith({ recover: [swap] });

    const result = await client.recoverSwaps();
    expect(result.swaps).toEqual([]);
    expect(await storage.get(swap.id)).toBeNull();
    expect(warnings).toMatchObject([
      {
        event: "client.recover.terms_refused",
        swapId: swap.id,
        data: { field },
      },
    ]);
  });

  it("refuses an invoice that does not decode and still recovers the rest", async () => {
    const { client, storage, warnings } = await clientWith({
      recover: [
        recovered(
          { ...L2E, bolt11_invoice: "lnbcrt1fixture" },
          "lightning_to_evm",
          7,
        ),
        recovered(L2A, "lightning_to_arkade"),
      ],
    });

    const result = await client.recoverSwaps();
    expect(result.swaps.map((swap) => swap.swapId)).toEqual([L2A.id]);
    expect(await storage.get(L2E.id)).toBeNull();
    expect(warnings).toMatchObject([
      { event: "client.recover.terms_refused", swapId: L2E.id },
    ]);
  });
});
