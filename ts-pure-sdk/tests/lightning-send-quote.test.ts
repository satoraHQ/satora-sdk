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
};

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
      sourceToken: "0x2791bca1f2de4661ed88a30c99a7a9449aa84174",
      sourceAmount: "42000000",
      lightningAddress: "user@example.test",
    });
    expect(result.sourceAmount).toBe("100250");
    expect(result.sourceAmountSats).toBeUndefined();
  });
});
