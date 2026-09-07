import { afterEach, describe, expect, it, vi } from "vitest";
import { Client } from "../src/index.js";

const quote = {
  source_amount: "100250",
  target_amount_sats: 100_000,
  protocol_fee_sats: 150,
  network_fee_sats: 100,
  protocol_fee_rate: 0.0015,
  min_amount_sats: 1_000,
  max_amount_sats: 10_000_000,
  exchange_rate: "1",
};

const USDC_POLYGON = "0x2791bca1f2de4661ed88a30c99a7a9449aa84174";
/** The DEX-priced USDC → Lightning quote: 42 USDC buys the 100_250-sat lock. */
const evmQuote = {
  ...quote,
  source_amount: "42000000",
  exchange_rate: "41895.26",
};

async function getQuoteWith(
  params: Parameters<Client["getQuote"]>[0],
  response: Record<string, unknown>,
) {
  const fetchMock = vi.fn(async (_input: RequestInfo | URL) =>
    Response.json(response, { status: 200 }),
  );
  vi.stubGlobal("fetch", fetchMock);
  const client = await Client.builder()
    .withBaseUrl("https://example.test")
    .build();
  const result = await client.getQuote(params);
  const urls = fetchMock.mock.calls.map(
    ([request]) => new URL((request as Request).url).pathname,
  );
  return { result, urls };
}

async function quoteWith(
  params: Parameters<Client["getLightningSendQuote"]>[0],
) {
  const fetchMock = vi.fn(async (_input: RequestInfo | URL) =>
    Response.json(quote, { status: 200 }),
  );
  vi.stubGlobal("fetch", fetchMock);
  const client = await Client.builder()
    .withBaseUrl("https://example.test")
    .build();
  const result = await client.getLightningSendQuote(params);
  const [request] = fetchMock.mock.calls[0] ?? [];
  const url = new URL((request as Request).url);
  return { result, query: url.searchParams };
}

describe("getLightningSendQuote", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("maps the deprecated sourceAmountSats param onto source_amount", async () => {
    const { query } = await quoteWith({
      lightningAddress: "user@example.test",
      sourceAmountSats: 100_250,
    });
    expect(query.get("source_amount")).toBe("100250");
  });

  it("prefers sourceAmount when both spellings are given", async () => {
    const { query } = await quoteWith({
      lightningAddress: "user@example.test",
      sourceAmount: 200_000n,
      sourceAmountSats: 100_250,
    });
    expect(query.get("source_amount")).toBe("200000");
  });

  it("mirrors sourceAmount as sourceAmountSats for Arkade sources", async () => {
    const { result } = await quoteWith({
      lightningAddress: "user@example.test",
      sourceAmount: 100_250,
    });
    expect(result.sourceAmount).toBe("100250");
    expect(result.sourceAmountSats).toBe(100_250);
  });

  it("leaves sourceAmountSats unset for EVM sources", async () => {
    const { result } = await quoteWith({
      sourceChain: "137",
      sourceToken: USDC_POLYGON,
      sourceAmount: "42000000",
      lightningAddress: "user@example.test",
    });
    expect(result.sourceAmount).toBe("100250");
    expect(result.sourceAmountSats).toBeUndefined();
  });

  it("passes the server's exchange rate through", async () => {
    const { result } = await quoteWith({
      lightningAddress: "user@example.test",
      sourceAmount: 100_250,
    });
    expect(result.exchangeRate).toBe("1");
  });
});

/// `getQuote` with a concrete Lightning destination is served from
/// `/quote/lightning-send`; the adapted `QuoteResponse` must carry that
/// endpoint's rate, not the Arkade 1:1 literal.
describe("getQuote via /quote/lightning-send", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("reports the DEX-derived rate for an EVM source", async () => {
    const { result, urls } = await getQuoteWith(
      {
        sourceChain: "137",
        sourceToken: USDC_POLYGON,
        targetChain: "Lightning",
        targetToken: "btc",
        targetAmount: 100_000,
        lightningDestination: "user@example.test",
      },
      evmQuote,
    );
    expect(urls).toEqual(["/quote/lightning-send"]);
    expect(result.exchange_rate).toBe("41895.26");
    expect(result.source_amount).toBe("42000000");
    expect(result.target_amount).toBe("100000");
  });

  it("keeps the 1:1 rate for an Arkade source", async () => {
    const { result, urls } = await getQuoteWith(
      {
        sourceChain: "Arkade",
        sourceToken: "btc",
        targetChain: "Lightning",
        targetToken: "btc",
        targetAmount: 100_000,
        lightningDestination: "user@example.test",
      },
      quote,
    );
    expect(urls).toEqual(["/quote/lightning-send"]);
    expect(result.exchange_rate).toBe("1");
  });
});
