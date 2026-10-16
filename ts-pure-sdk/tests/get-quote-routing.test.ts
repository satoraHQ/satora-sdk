import { afterEach, describe, expect, it, vi } from "vitest";
import { Client } from "../src/index.js";

const USDC_POLYGON = "0x2791bca1f2de4661ed88a30c99a7a9449aa84174";

/** A server-composed `/quote` body; its values are opaque to the routing test. */
const serverQuote = {
  exchange_rate: "server",
  network_fee: 1,
  gasless_network_fee: 0,
  protocol_fee: 2,
  protocol_fee_rate: 0.002,
  min_amount: 1,
  max_amount: 1_000_000_000,
  source_amount: "100000",
  target_amount: "80000000",
  net_source_amount: "100000",
  net_target_amount: "79990000",
};

async function getQuoteWith(
  params: Parameters<Client["getQuote"]>[0],
  referralCode?: string,
) {
  const fetchMock = vi.fn(async (_input: RequestInfo | URL) =>
    Response.json(serverQuote, { status: 200 }),
  );
  vi.stubGlobal("fetch", fetchMock);
  let builder = Client.builder().withBaseUrl("https://example.test");
  if (referralCode) builder = builder.withReferralCode(referralCode);
  const client = await builder.build();
  const result = await client.getQuote(params);
  const requests = fetchMock.mock.calls.map(([request]) => {
    const url = new URL((request as Request).url);
    return { path: url.pathname, ref: url.searchParams.get("ref") };
  });
  return { result, requests };
}

const btcToUsdc = {
  sourceChain: "Bitcoin" as const,
  sourceToken: "btc",
  targetChain: "137" as const,
  targetToken: USDC_POLYGON,
  sourceAmount: 100_000,
};

describe("getQuote routing under a referral code", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("a request-level referral code goes straight to /quote", async () => {
    const { result, requests } = await getQuoteWith({
      ...btcToUsdc,
      referralCode: "partner",
    });
    expect(requests).toEqual([{ path: "/quote", ref: "partner" }]);
    expect(result.exchange_rate).toBe("server");
  });

  it("a client-level referral code goes straight to /quote", async () => {
    const { requests } = await getQuoteWith(btcToUsdc, "default");
    expect(requests).toEqual([{ path: "/quote", ref: "default" }]);
  });
});
