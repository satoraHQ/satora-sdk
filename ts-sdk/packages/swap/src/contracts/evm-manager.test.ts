import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HtlcObservation } from "../actions/types.js";
import type { EvmHtlcEvent } from "./evm.js";
import {
  type EvmActiveQuery,
  type EvmChainReader,
  EvmContractManager,
  type EvmHtlcQuery,
  htlcQueryKey,
  refQueryKey,
} from "./evm-manager.js";
import type { HtlcRef } from "./types.js";

const ref = {
  ledger: "evm",
  chainId: 137,
  htlc: "0xhtlc",
  preimageHash: "0xph",
  claimAddress: "0xclaim",
  expectedAmount: 1000n,
  expectedToken: "0xwbtc",
} satisfies HtlcRef;

/** Mirrors how the real reader keys an `isActiveBatch` result. */
function activeKey(q: EvmActiveQuery): string {
  return htlcQueryKey({
    htlc: q.htlc,
    preimageHash: q.preimageHash,
    claimAddress: q.claimAddress,
    terms: {
      amount: q.amount,
      token: q.token,
      sender: q.sender,
      timelockSec: q.timelockSec,
    },
  });
}

class FakeReader implements EvmChainReader {
  /** Events served for every queried HTLC (the log path). */
  events: EvmHtlcEvent[] = [];
  /** activeKey → open? Missing key reads as inactive. */
  active = new Map<string, boolean>();
  blockTimeMs = 1_000;
  blockNumber = 100n;
  getHtlcEventsBatch = vi.fn(
    async (queries: EvmHtlcQuery[], _fromBlock?: bigint) => {
      return new Map(queries.map((q) => [htlcQueryKey(q), this.events]));
    },
  );
  isActiveBatch = vi.fn(async (queries: EvmActiveQuery[]) => {
    return new Map(
      queries.map((q) => [
        activeKey(q),
        this.active.get(activeKey(q)) ?? false,
      ]),
    );
  });
  getLatestBlock = async () => ({
    timeMs: this.blockTimeMs,
    number: this.blockNumber,
  });
}

describe("EvmContractManager", () => {
  let reader: FakeReader;
  let readers: Map<number, EvmChainReader>;

  beforeEach(() => {
    reader = new FakeReader();
    readers = new Map([[137, reader]]);
  });

  const build = () => EvmContractManager.fromDeps({ readers });

  it("rejects non-evm HTLCs", async () => {
    await expect(
      build().register({ ledger: "lightning", paymentHash: "ab" }),
    ).rejects.toThrow(/can't track/);
  });

  it("throws for a chain with no configured reader (instead of silently stalling)", async () => {
    await expect(build().register({ ...ref, chainId: 8453 })).rejects.toThrow(
      /no EVM reader for chain 8453/,
    );
  });

  it("keeps observing one chain when another chain's endpoint fails", async () => {
    // Every EVM chain shares this one manager, so a swap on a healthy chain must
    // not go unobserved because an unrelated swap's chain is unreachable.
    const broken = new FakeReader();
    const down = () => {
      throw new Error("range 44268 exceeds limit of 10000");
    };
    broken.getHtlcEventsBatch = vi.fn(down);
    broken.isActiveBatch = vi.fn(down);
    broken.getLatestBlock = down;

    reader.events = [{ kind: "created", amount: 1000n, token: "0xwbtc" }];
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const m = EvmContractManager.fromDeps({
      readers: new Map<number, EvmChainReader>([
        [1, broken],
        [137, reader],
      ]),
    });
    await m.register({ ...ref, chainId: 1 });
    await m.register(ref);

    await expect(m.refresh()).resolves.toBeUndefined();

    expect(m.getState(ref)).toBe("confirmed");
    expect(warn).toHaveBeenCalledWith(
      "EVM reconcile failed for chain 1:",
      expect.any(Error),
    );
    warn.mockRestore();
  });

  it("register makes no RPC calls; the first refresh seeds observation and clock", async () => {
    const m = build();
    reader.events = [{ kind: "created", amount: 1000n, token: "0xwbtc" }];
    await m.register(ref);
    // Registration is passive — a startup burst of registers must not fan out
    // into per-swap chain scans. The tracker always follows with refresh().
    expect(reader.getHtlcEventsBatch).not.toHaveBeenCalled();
    expect(reader.isActiveBatch).not.toHaveBeenCalled();
    expect(m.getState(ref)).toBeUndefined();

    await m.refresh();
    expect(m.getState(ref)).toBe("confirmed");
    expect(m.chainNow(ref)).toBeGreaterThanOrEqual(1_000);
    expect(reader.getHtlcEventsBatch).toHaveBeenCalledWith(
      [{ htlc: "0xhtlc", preimageHash: "0xph", claimAddress: "0xclaim" }],
      0n,
    );
  });

  it("reads a whole chain's HTLCs in one batched call", async () => {
    const m = build();
    const ref2 = { ...ref, preimageHash: "0xph2" } satisfies HtlcRef;
    await m.register(ref);
    await m.register(ref2);

    reader.getHtlcEventsBatch.mockClear();
    await m.refresh();

    expect(reader.getHtlcEventsBatch).toHaveBeenCalledTimes(1);
    const queries = reader.getHtlcEventsBatch.mock.calls[0][0];
    expect(queries.map((q) => q.preimageHash).sort()).toEqual([
      "0xph",
      "0xph2",
    ]);
  });

  it("scans unknown-age refs separately so they don't drag the batch to genesis", async () => {
    const m = build();
    reader.blockTimeMs = 10_000_000;
    reader.blockNumber = 50_000n;
    const knownRef = {
      ...ref,
      preimageHash: "0xknown",
      createdAtMs: 9_000_000,
    } satisfies HtlcRef;
    const legacyRef = { ...ref, preimageHash: "0xlegacy" } satisfies HtlcRef;
    await m.register(knownRef);
    await m.register(legacyRef);
    await m.refresh();

    // Two batches: the known-age ref keeps its near-creation lower bound; the
    // legacy ref (no created_at) still gets the full-range scan it needs —
    // bounding it by the other ref's estimate could miss its events entirely.
    expect(reader.getHtlcEventsBatch).toHaveBeenCalledTimes(2);
    const calls = reader.getHtlcEventsBatch.mock.calls;
    const bounded = calls.find((c) =>
      c[0].some((q) => q.preimageHash === "0xknown"),
    );
    const genesis = calls.find((c) =>
      c[0].some((q) => q.preimageHash === "0xlegacy"),
    );
    expect(bounded?.[0]).toHaveLength(1);
    expect(bounded?.[1]).toBeGreaterThan(0n);
    expect(genesis?.[0]).toHaveLength(1);
    expect(genesis?.[1]).toBe(0n);
  });

  it("is invalid when the HTLC is funded below the expected amount", async () => {
    const m = build();
    reader.events = [{ kind: "created", amount: 999n, token: "0xwbtc" }];
    await m.register(ref);
    await m.refresh();
    expect(m.getState(ref)).toBe("invalid");
  });

  it("minAmount, not expectedAmount, is the invalid threshold when set", async () => {
    const m = build();
    reader.events = [{ kind: "created", amount: 999n, token: "0xwbtc" }];
    await m.register({ ...ref, minAmount: 0n });
    await m.refresh();
    expect(m.getState({ ...ref, minAmount: 0n })).toBe("confirmed");
  });

  it("re-observes and notifies on refresh", async () => {
    const m = build();
    const seen: HtlcObservation[] = [];
    m.onEvent((_r, s) => seen.push(s));
    reader.events = [{ kind: "created", amount: 1000n, token: "0xwbtc" }];
    await m.register(ref);
    await m.refresh();
    expect(m.getState(ref)).toBe("confirmed");

    reader.events = [
      { kind: "created", amount: 1000n, token: "0xwbtc" },
      { kind: "redeemed", preimage: "0xse" },
    ];
    await m.refresh();
    expect(m.getState(ref)).toBe("spent_claim");
    expect(m.getPreimage(ref)).toBe("0xse");
    expect(seen).toEqual(["confirmed", "spent_claim"]);
  });

  it("never downgrades a resolved spend", async () => {
    const m = build();
    reader.events = [
      { kind: "created", amount: 1000n, token: "0xwbtc" },
      { kind: "refunded" },
    ];
    await m.register(ref);
    await m.refresh();
    expect(m.getState(ref)).toBe("spent_refund");
    // A stale read that no longer sees the refund must not revert it.
    reader.events = [{ kind: "created", amount: 1000n, token: "0xwbtc" }];
    await m.refresh();
    expect(m.getState(ref)).toBe("spent_refund");
  });

  it("tracks independent clocks per chain", async () => {
    const other = new FakeReader();
    other.blockTimeMs = 500_000;
    readers.set(1, other);
    const m = build();
    const ethRef = { ...ref, chainId: 1 } satisfies HtlcRef;
    await m.register(ref);
    await m.register(ethRef);
    await m.refresh();
    expect(m.chainNow(ref) ?? 0).toBeLessThan(m.chainNow(ethRef) ?? 0);
  });

  it("clears a chain's state once its last HTLC is unregistered", async () => {
    const m = build();
    await m.register(ref);
    await m.unregister(ref);
    expect(m.getState(ref)).toBeUndefined();
    expect(m.chainNow(ref)).toBeUndefined();
  });

  describe("isActive fast path (complete tuple)", () => {
    // A ref whose response exposed the whole contract tuple.
    const tupleRef = {
      ...ref,
      sender: "0xsender",
      timelockSec: 1_700_000_000,
      createdAtMs: 500,
    } satisfies HtlcRef;

    it("confirms an active HTLC without any log scan", async () => {
      const m = build();
      reader.active.set(refQueryKey(tupleRef), true);
      await m.register(tupleRef);
      await m.refresh();
      expect(m.getState(tupleRef)).toBe("confirmed");
      // isActive == true proves the terms — no logs needed at all.
      expect(reader.getHtlcEventsBatch).not.toHaveBeenCalled();
    });

    it("classifies an inactive HTLC from logs", async () => {
      const m = build();
      reader.events = [
        { kind: "created", amount: 1000n, token: "0xwbtc" },
        { kind: "refunded" },
      ];
      await m.register(tupleRef); // active-map empty → inactive
      await m.refresh();
      expect(reader.getHtlcEventsBatch).toHaveBeenCalledTimes(1);
      expect(m.getState(tupleRef)).toBe("spent_refund");
    });

    it("falls back to logs when the isActive check fails (no Multicall on chain)", async () => {
      const m = build();
      reader.isActiveBatch.mockRejectedValueOnce(new Error("no multicall"));
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
      reader.events = [{ kind: "created", amount: 1000n, token: "0xwbtc" }];
      await m.register(tupleRef);
      await m.refresh();
      expect(m.getState(tupleRef)).toBe("confirmed");
      warn.mockRestore();
    });

    it("stops scanning a leg once it is latched on a spend", async () => {
      const m = build();
      reader.events = [
        { kind: "created", amount: 1000n, token: "0xwbtc" },
        { kind: "refunded" },
      ];
      await m.register(tupleRef);
      await m.refresh();
      expect(m.getState(tupleRef)).toBe("spent_refund");

      reader.isActiveBatch.mockClear();
      reader.getHtlcEventsBatch.mockClear();
      await m.refresh();
      // Terminal per leg: nothing left to learn, no requests at all.
      expect(reader.isActiveBatch).not.toHaveBeenCalled();
      expect(reader.getHtlcEventsBatch).not.toHaveBeenCalled();
    });
  });

  describe("with fake time", () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it("a targeted reconcile hits the chain immediately", async () => {
      const m = build();
      await m.register(ref);
      reader.getHtlcEventsBatch.mockClear();

      await m.reconcile(ref); // hint / at-risk path
      await m.reconcile(ref);
      expect(reader.getHtlcEventsBatch).toHaveBeenCalledTimes(2);
    });

    it("extrapolates chainNow between reads", async () => {
      const m = build();
      await m.register(ref);
      await m.refresh();
      const at0 = m.chainNow(ref);
      vi.advanceTimersByTime(30_000);
      // No RPC in between — the clock still advances with wall time.
      expect(m.chainNow(ref)).toBe((at0 ?? 0) + 30_000);
    });
  });
});
