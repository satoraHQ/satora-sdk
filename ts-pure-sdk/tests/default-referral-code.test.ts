import { afterEach, describe, expect, it, vi } from "vitest";
import {
  Client,
  InMemorySwapStorage,
  InMemoryWalletStorage,
} from "../src/index.js";
import vectors from "./fixtures/lightning-invoices.js";

const { swaps } = vectors;
const L2A = swaps.lightning_to_arkade.response;
const A2L = swaps.arkade_to_lightning.response;
const A2L_INVOICE = A2L.client_lightning_invoice;
const BASE_URL = "http://swap.test";
const DEFAULT_REF = "partner-default";

const QUOTE = {
  exchange_rate: "1",
  network_fee: 2,
  gasless_network_fee: 0,
  protocol_fee: 60,
  protocol_fee_rate: 0.003,
  min_amount: 1_000,
  max_amount: 1_000_000,
  source_amount: "20000",
  target_amount: "20000",
  net_source_amount: "20000",
  net_target_amount: "19938",
};

interface Seen {
  path: string;
  query: URLSearchParams;
  body?: Record<string, unknown>;
}

/**
 * A client built with `withReferralCode(DEFAULT_REF)` whose server answers
 * creates with the regtest daemon's real responses and quotes with QUOTE,
 * recording every request.
 */
async function referralClient() {
  const seen: Seen[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = new Request(input, init);
      const url = new URL(request.url);
      const text = request.method === "POST" ? await request.text() : "";
      seen.push({
        path: url.pathname,
        query: url.searchParams,
        body: text ? JSON.parse(text) : undefined,
      });
      const answer = (body: unknown) => Response.json(body, { status: 200 });
      if (url.pathname === "/swap/lightning/arkade") return answer(L2A);
      if (url.pathname === "/swap/arkade/lightning") return answer(A2L);
      if (url.pathname === "/quote") return answer(QUOTE);
      throw new Error(`unexpected request ${request.method} ${url.pathname}`);
    }),
  );
  const client = await Client.builder()
    .withBaseUrl(BASE_URL)
    .withMnemonic(vectors.mnemonic)
    .withSignerStorage(new InMemoryWalletStorage())
    .withSwapStorage(new InMemorySwapStorage())
    .withReferralCode(DEFAULT_REF)
    .build();
  // The fixture responses were created at key index 1.
  await client.setKeyIndex(1);
  const created = (path: string) =>
    seen.filter((s) => s.path === path).map((s) => s.body?.referral_code);
  return { client, seen, created };
}

afterEach(() => vi.unstubAllGlobals());

describe("the client-level default referral code", () => {
  it("is sent by a direct createArkadeToLightningSwap", async () => {
    const { client, created } = await referralClient();
    await client.createArkadeToLightningSwap({ lightningInvoice: A2L_INVOICE });
    expect(created("/swap/arkade/lightning")).toEqual([DEFAULT_REF]);
  });

  it("is sent by a direct createLightningToArkadeSwap", async () => {
    const { client, created } = await referralClient();
    await client.createLightningToArkadeSwap({
      targetAddress: L2A.target_arkade_address,
      sourceAmountSats: 20000,
    });
    expect(created("/swap/lightning/arkade")).toEqual([DEFAULT_REF]);
  });

  it("loses to a per-call referralCode", async () => {
    const { client, created } = await referralClient();
    await client.createArkadeToLightningSwap({
      lightningInvoice: A2L_INVOICE,
      referralCode: "per-call",
    });
    expect(created("/swap/arkade/lightning")).toEqual(["per-call"]);
  });

  it("prices the quote and the direct create with the same referral", async () => {
    const { client, seen, created } = await referralClient();
    await client.getQuote({
      sourceChain: "Arkade",
      sourceToken: "btc",
      targetChain: "Lightning",
      targetToken: "btc",
      sourceAmount: 20000,
    });
    await client.createArkadeToLightningSwap({ lightningInvoice: A2L_INVOICE });
    const quoteRefs = seen
      .filter((s) => s.path === "/quote")
      .map((s) => s.query.get("ref"));
    expect(quoteRefs).toEqual([DEFAULT_REF]);
    expect(created("/swap/arkade/lightning")).toEqual(quoteRefs);
  });
});
