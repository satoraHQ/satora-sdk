import { describe, expect, it, vi } from "vitest";
import {
  esploraReader,
  htlcFactsFromEsploraTxs,
} from "./bitcoin-reader-esplora.js";

const ADDR = "bcrt1qhtlc";

// biome-ignore lint/suspicious/noExplicitAny: minimal esplora tx fixtures
const fundingTx = (confirmed: boolean, value = 5000): any => ({
  vin: [{ prevout: { scriptpubkey_address: "bcrt1qsomeoneelse" } }],
  vout: [{ scriptpubkey_address: ADDR, value }],
  status: { confirmed },
});
// biome-ignore lint/suspicious/noExplicitAny: minimal esplora tx fixtures
const spendTx = (witness: string[]): any => ({
  vin: [{ witness, prevout: { scriptpubkey_address: ADDR } }],
  vout: [{ scriptpubkey_address: "bcrt1qdestination" }],
  status: { confirmed: true },
});

describe("htlcFactsFromEsploraTxs", () => {
  it("is absent with no txs", () => {
    expect(htlcFactsFromEsploraTxs([], ADDR)).toEqual({
      funding: "absent",
      fundedSats: 0,
    });
  });

  it("is mempool for an unconfirmed funding tx, with the funded amount", () => {
    expect(htlcFactsFromEsploraTxs([fundingTx(false)], ADDR)).toEqual({
      funding: "mempool",
      fundedSats: 5000,
    });
  });

  it("is confirmed for a confirmed funding tx, summing the outputs to us", () => {
    expect(htlcFactsFromEsploraTxs([fundingTx(true)], ADDR)).toEqual({
      funding: "confirmed",
      fundedSats: 5000,
    });
  });

  it("returns the spend witness once the HTLC output is spent", () => {
    const witness = ["3045ab", "aabbcc"];
    expect(
      htlcFactsFromEsploraTxs([fundingTx(true), spendTx(witness)], ADDR),
    ).toEqual({ funding: "confirmed", fundedSats: 0, spendWitness: witness });
  });
});

describe("esploraReader", () => {
  it("fetches the address txs and reduces them to facts", async () => {
    const fetchImpl = vi.fn(async () => ({
      ok: true,
      json: async () => [fundingTx(true)],
    })) as unknown as typeof fetch;
    const reader = esploraReader("http://esplora/api/", fetchImpl);
    expect(await reader.getHtlcFacts(ADDR)).toEqual({
      funding: "confirmed",
      fundedSats: 5000,
    });
    expect(fetchImpl).toHaveBeenCalledWith(
      `http://esplora/api/address/${ADDR}/txs`,
    );
  });

  it("throws when the only endpoint is non-ok", async () => {
    const fetchImpl = vi.fn(async () => ({
      ok: false,
      status: 404,
    })) as unknown as typeof fetch;
    await expect(
      esploraReader("http://esplora/api", fetchImpl).getHtlcFacts(ADDR),
    ).rejects.toThrow(/esplora 404/);
  });

  it("fails over to the next endpoint when one errors", async () => {
    const fetchImpl = vi.fn(async (url: string) => {
      if (url.startsWith("http://primary")) throw new Error("throttled"); // primary down
      return { ok: true, json: async () => [fundingTx(true)] };
    }) as unknown as typeof fetch;
    const reader = esploraReader(
      ["http://primary/api", "http://backup/api"],
      fetchImpl,
    );
    expect(await reader.getHtlcFacts(ADDR)).toEqual({
      funding: "confirmed",
      fundedSats: 5000,
    });
    expect(fetchImpl).toHaveBeenCalledWith(
      `http://backup/api/address/${ADDR}/txs`,
    );
  });

  it("rotates the primary endpoint across calls to spread load", async () => {
    const hits: string[] = [];
    const fetchImpl = vi.fn(async (url: string) => {
      hits.push(new URL(url).host);
      return { ok: true, json: async () => [] };
    }) as unknown as typeof fetch;
    const reader = esploraReader(["http://a/api", "http://b/api"], fetchImpl);
    await reader.getHtlcFacts(ADDR);
    await reader.getHtlcFacts(ADDR);
    expect(hits).toEqual(["a", "b"]); // first call → a, second → b
  });
});
