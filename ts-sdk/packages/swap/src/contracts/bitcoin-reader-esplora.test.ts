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

  it("accepts an unconfirmed (non-RBF) funding as confirmed by default (0-conf)", () => {
    expect(htlcFactsFromEsploraTxs([fundingTx(false)], ADDR)).toEqual({
      funding: "confirmed",
      fundedSats: 5000,
    });
  });

  it("accepts an RBF-signaling funding at 0-conf too (funder is trusted)", () => {
    // 0-conf inherently trusts the funder not to double-spend; RBF signalling
    // doesn't change that (full-RBF miners replace non-signalling txs anyway),
    // so it isn't a separate gate. Use minConfirmations >= 1 to not trust.
    const rbf = fundingTx(false);
    rbf.vin[0].sequence = 0xfffffffd; // < 0xfffffffe ⟹ replaceable
    expect(htlcFactsFromEsploraTxs([rbf], ADDR)).toEqual({
      funding: "confirmed",
      fundedSats: 5000,
    });
    expect(
      htlcFactsFromEsploraTxs([rbf], ADDR, { minConfirmations: 1 }).funding,
    ).toBe("mempool");
  });

  it("minConfirmations: 1 restores the strict wait-for-a-block behavior", () => {
    expect(
      htlcFactsFromEsploraTxs([fundingTx(false)], ADDR, {
        minConfirmations: 1,
      }),
    ).toEqual({ funding: "mempool", fundedSats: 5000 });
    expect(
      htlcFactsFromEsploraTxs([fundingTx(true)], ADDR, { minConfirmations: 1 })
        .funding,
    ).toBe("confirmed");
  });

  it("minConfirmations > 1 requires the depth (computed from tipHeight)", () => {
    const confirmedAt100 = fundingTx(true);
    confirmedAt100.status.block_height = 100;
    const at = (tipHeight?: number) =>
      htlcFactsFromEsploraTxs([confirmedAt100], ADDR, {
        minConfirmations: 2,
        tipHeight,
      }).funding;
    expect(at(100)).toBe("mempool"); // 1 conf < 2
    expect(at(101)).toBe("confirmed"); // 2 confs
    expect(at(undefined)).toBe("mempool"); // unknown tip → don't overclaim depth
  });

  it("is confirmed for a confirmed funding tx, summing the outputs to us", () => {
    expect(htlcFactsFromEsploraTxs([fundingTx(true)], ADDR)).toEqual({
      funding: "confirmed",
      fundedSats: 5000,
    });
  });

  it("picks the tx paying the most, not the first listed", () => {
    // Esplora lists address txs newest-first, and the address is public —
    // a stray dust payment listed before the real funding must not observe
    // the swap as underfunded.
    expect(
      htlcFactsFromEsploraTxs([fundingTx(false, 330), fundingTx(true)], ADDR),
    ).toEqual({
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

  it("re-reads a getter policy on every call", async () => {
    const fetchImpl = vi.fn(async () => ({
      ok: true,
      json: async () => [fundingTx(false)], // unconfirmed
    })) as unknown as typeof fetch;
    let depth: number | undefined = 0;
    const reader = esploraReader("http://esplora/api", fetchImpl, {
      minConfirmations: () => depth,
    });

    expect((await reader.getHtlcFacts(ADDR)).funding).toBe("confirmed");
    depth = 1;
    expect((await reader.getHtlcFacts(ADDR)).funding).toBe("mempool");
    depth = undefined; // getter returning undefined falls back to 0
    expect((await reader.getHtlcFacts(ADDR)).funding).toBe("confirmed");
  });

  it("lets a per-call depth override the getter policy", async () => {
    const fetchImpl = vi.fn(async () => ({
      ok: true,
      json: async () => [fundingTx(false)],
    })) as unknown as typeof fetch;
    const reader = esploraReader("http://esplora/api", fetchImpl, {
      minConfirmations: () => 0,
    });
    expect((await reader.getHtlcFacts(ADDR, 1)).funding).toBe("mempool");
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

  it("fetches the tip height only for depth policies beyond one confirmation", async () => {
    const confirmedAt100 = fundingTx(true);
    confirmedAt100.status.block_height = 100;
    const fetchImpl = vi.fn(async (url: string) => {
      if (url.endsWith("/blocks/tip/height"))
        return { ok: true, text: async () => "101" };
      return { ok: true, json: async () => [confirmedAt100] };
    }) as unknown as typeof fetch;
    const reader = esploraReader("http://esplora/api", fetchImpl, {
      minConfirmations: 2,
    });
    expect((await reader.getHtlcFacts(ADDR)).funding).toBe("confirmed");
    expect(fetchImpl).toHaveBeenCalledWith(
      "http://esplora/api/blocks/tip/height",
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
