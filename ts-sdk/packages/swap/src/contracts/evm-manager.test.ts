import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HtlcObservation } from "../actions/types.js";
import type { EvmHtlcEvent } from "./evm.js";
import { type EvmChainReader, EvmContractManager } from "./evm-manager.js";
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

class FakeReader implements EvmChainReader {
  events: EvmHtlcEvent[] = [];
  blockTimeMs = 1_000;
  #cb: (() => void) | undefined;
  getHtlcEvents = vi.fn(async () => this.events);
  getBlockTimeMs = async () => this.blockTimeMs;
  watch = (cb: () => void): (() => void) => {
    this.#cb = cb;
    return () => {
      this.#cb = undefined;
    };
  };
  /** Simulate a new block/log notification. */
  fire(): void {
    this.#cb?.();
  }
  get watching(): boolean {
    return this.#cb !== undefined;
  }
}

/** Let fire-and-forget async reconciliation settle. */
const tick = () => new Promise<void>((r) => setTimeout(r, 0));

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

  it("seeds the observation and the chain clock on register", async () => {
    const m = build();
    reader.events = [{ kind: "created", amount: 1000n, token: "0xwbtc" }];
    await m.register(ref);
    expect(m.getState(ref)).toBe("confirmed");
    expect(m.chainNow(ref)).toBe(1_000);
    expect(reader.getHtlcEvents).toHaveBeenCalledWith(
      "0xhtlc",
      "0xph",
      "0xclaim",
    );
  });

  it("is invalid when the HTLC is funded below the expected amount", async () => {
    const m = build();
    reader.events = [{ kind: "created", amount: 999n, token: "0xwbtc" }];
    await m.register(ref);
    expect(m.getState(ref)).toBe("invalid");
  });

  it("re-observes and notifies when the reader signals a change", async () => {
    const m = build();
    const seen: HtlcObservation[] = [];
    m.onEvent((_r, s) => seen.push(s));
    reader.events = [{ kind: "created", amount: 1000n, token: "0xwbtc" }];
    await m.register(ref);
    expect(m.getState(ref)).toBe("confirmed");

    reader.events = [
      { kind: "created", amount: 1000n, token: "0xwbtc" },
      { kind: "redeemed", preimage: "0xse" },
    ];
    reader.fire();
    await tick();
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
    expect(m.getState(ref)).toBe("spent_refund");
    // A stale read that no longer sees the refund must not revert it.
    reader.events = [{ kind: "created", amount: 1000n, token: "0xwbtc" }];
    reader.fire();
    await tick();
    expect(m.getState(ref)).toBe("spent_refund");
  });

  it("tracks independent clocks per chain", async () => {
    const other = new FakeReader();
    other.blockTimeMs = 2_000;
    readers.set(1, other);
    const m = build();
    const ethRef = { ...ref, chainId: 1 } satisfies HtlcRef;
    await m.register(ref);
    await m.register(ethRef);
    expect(m.chainNow(ref)).toBe(1_000);
    expect(m.chainNow(ethRef)).toBe(2_000);
  });

  it("stops watching a chain once its last HTLC is unregistered", async () => {
    const m = build();
    await m.register(ref);
    expect(reader.watching).toBe(true);
    await m.unregister(ref);
    expect(reader.watching).toBe(false);
    expect(m.getState(ref)).toBeUndefined();
  });

  it("disposes all chain watches", async () => {
    const m = build();
    await m.register(ref);
    m.dispose();
    expect(reader.watching).toBe(false);
  });
});
