import { bech32m } from "@scure/base";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GetSwapResponse } from "../src/api/client.js";
import { Client } from "../src/client.js";
import { createSparkToEvmSwap } from "../src/create/spark-to-evm.js";
import type { CreateSwapContext } from "../src/create/types.js";
import {
  InMemorySwapStorage,
  InMemoryWalletStorage,
} from "../src/storage/index.js";

const mnemonic =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const address = bech32m.encode(
  "sparkrt",
  bech32m.toWords(new Uint8Array(33).fill(2)),
  1023,
);

async function fixture(
  mutate: (data: Record<string, unknown>) => void = () => {},
) {
  const client = await Client.builder().withMnemonic(mnemonic).build();
  const params = client.deriveSwapParamsAtIndex(7);
  const storeSwap = vi.fn();
  const post = vi.fn(
    async (_path: string, { body }: { body: Record<string, unknown> }) => {
      const data = {
        id: "11111111-2222-3333-4444-555555555555",
        network: "regtest",
        hash_lock: body.hash_lock,
        source_amount: "20000",
        funding: { address, expiry_duration_secs: 3600, payment_id: null },
        client_evm_address: body.claiming_address,
        target_evm_address: body.target_address,
      };
      mutate(data);
      return {
        data,
        error: undefined,
        response: new Response(null, { status: 200 }),
      };
    },
  );
  const ctx = {
    apiClient: { POST: post },
    deriveSwapParams: async () => params,
    storeSwap,
  } as unknown as CreateSwapContext;
  const create = () =>
    createSparkToEvmSwap(
      {
        targetAddress: "0x000000000000000000000000000000000000dEaD",
        tokenAddress: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
        evmChainId: 42161,
        sourceAmount: 20000,
      },
      ctx,
    );
  return { create, post, storeSwap };
}

describe("native Spark swap creation", () => {
  it("uses native funding instructions and stores a distinct direction", async () => {
    const { create, post, storeSwap } = await fixture();
    const result = await create();
    expect(post.mock.calls[0][0]).toBe("/swap/spark/evm");
    expect(result.response.funding.address).toBe(address);
    expect(storeSwap.mock.calls[0][2].direction).toBe("spark_to_evm");
    expect(post.mock.calls[0][1].body).not.toHaveProperty("preimage");
  });
  it.each([
    [
      "hash",
      (data: Record<string, unknown>) => {
        data.hash_lock = `0x${"00".repeat(32)}`;
      },
    ],
    [
      "amount",
      (data: Record<string, unknown>) => {
        data.source_amount = "21000";
      },
    ],
    [
      "network",
      (data: Record<string, unknown>) => {
        data.network = "bitcoin";
      },
    ],
    [
      "ordinary address",
      (data: Record<string, unknown>) => {
        data.funding = {
          address: "not-a-spark-address",
          expiry_duration_secs: 3600,
        };
      },
    ],
    [
      "expiry",
      (data: Record<string, unknown>) => {
        data.funding = { address, expiry_duration_secs: 0 };
      },
    ],
  ] as const)("refuses mismatched %s before storage", async (_, mutate) => {
    const { create, storeSwap } = await fixture(mutate);
    await expect(create()).rejects.toThrow();
    expect(storeSwap).not.toHaveBeenCalled();
  });
});

afterEach(() => vi.unstubAllGlobals());

type NativeResponse = Extract<GetSwapResponse, { direction: "spark_to_evm" }>;

async function refreshFixture() {
  let refresh: GetSwapResponse;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = new Request(input, init);
      const path = new URL(request.url).pathname;
      if (request.method === "POST" && path === "/swap/spark/evm") {
        const body = await request.json();
        return Response.json({
          id: "11111111-2222-3333-4444-555555555555",
          status: "pending",
          network: "regtest",
          hash_lock: body.hash_lock,
          source_amount: "20000",
          target_amount: "1000000",
          funding: { address, expiry_duration_secs: 3600, payment_id: null },
          client_evm_address: body.claiming_address,
          target_evm_address: body.target_address,
        });
      }
      if (
        request.method === "GET" &&
        path === "/swap/11111111-2222-3333-4444-555555555555"
      ) {
        return Response.json(refresh);
      }
      throw new Error(`Unexpected request ${request.method} ${path}`);
    }),
  );
  const storage = new InMemorySwapStorage();
  const warn = vi.fn();
  const client = await Client.builder()
    .withBaseUrl("https://api.test")
    .withMnemonic(mnemonic)
    .withSignerStorage(new InMemoryWalletStorage())
    .withSwapStorage(storage)
    .withLogger({ warn })
    .withLogLevel("warn")
    .build();
  const { response } = await client.createSparkToEvmSwap({
    targetAddress: "0x000000000000000000000000000000000000dEaD",
    tokenAddress: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
    evmChainId: 42161,
    sourceAmount: 20000,
  });
  const original: NativeResponse = { ...response, direction: "spark_to_evm" };
  return {
    client,
    storage,
    original,
    warn,
    setRefresh: (next: GetSwapResponse) => {
      refresh = next;
    },
  };
}

describe("native Spark refresh preserves funding terms", () => {
  const mutations: [string, (fresh: NativeResponse) => void][] = [
    [
      "funding.address",
      (fresh) => {
        fresh.funding.address = bech32m.encode(
          "sparkrt",
          bech32m.toWords(new Uint8Array(33).fill(3)),
          1023,
        );
      },
    ],
    [
      "funding.expiry_duration_secs",
      (fresh) => {
        fresh.funding.expiry_duration_secs = 86_400;
      },
    ],
    [
      "source_amount",
      (fresh) => {
        fresh.source_amount = "40000";
      },
    ],
    [
      "network",
      (fresh) => {
        fresh.network = "bitcoin";
      },
    ],
    [
      "multiple terms",
      (fresh) => {
        fresh.funding.address = "changed-recipient";
        fresh.funding.expiry_duration_secs = 86_400;
        fresh.source_amount = "40000";
        fresh.network = "bitcoin";
      },
    ],
  ];

  it.each(
    mutations,
  )("keeps original %s in returned and stored data across repeated refreshes", async (_, mutate) => {
    const { client, storage, original, warn, setRefresh } =
      await refreshFixture();
    for (const status of ["clientfunded", "serverfunded"] as const) {
      const fresh = structuredClone(original);
      fresh.status = status;
      fresh.funding.payment_id = "selected-transfer";
      mutate(fresh);
      setRefresh(fresh);
      const read = await client.getSwap(original.id, { updateStorage: true });
      const expected = {
        ...original,
        status,
        funding: { ...original.funding, payment_id: "selected-transfer" },
      };
      expect(read).toEqual(expected);
      expect((await storage.get(original.id))?.response).toEqual(expected);
    }
    // A rejected refresh must not become the baseline for the next refresh.
    expect(warn).toHaveBeenCalledTimes(2);
  });

  it("preserves native terms when a changed direction triggers refusal first", async () => {
    const { client, storage, original, setRefresh } = await refreshFixture();
    setRefresh({
      ...original,
      direction: "lightning_to_evm",
      status: "serverfunded",
      source_amount: "40000",
      network: "bitcoin",
      funding: {
        address: "changed-recipient",
        expiry_duration_secs: 86_400,
        payment_id: "unrelated-payment",
      },
    } as unknown as GetSwapResponse);
    const expected = { ...original, status: "serverfunded" };
    expect(await client.getSwap(original.id, { updateStorage: true })).toEqual(
      expected,
    );
    expect((await storage.get(original.id))?.response).toEqual(expected);
  });

  it("accepts legitimate status and selected-payment updates", async () => {
    const { client, storage, original, warn, setRefresh } =
      await refreshFixture();
    const fresh = structuredClone(original);
    fresh.status = "serverfunded";
    fresh.funding.payment_id = "selected-transfer";
    setRefresh(fresh);
    expect(await client.getSwap(original.id, { updateStorage: true })).toEqual(
      fresh,
    );
    expect((await storage.get(original.id))?.response).toEqual(fresh);
    expect(warn).not.toHaveBeenCalled();
  });
});
