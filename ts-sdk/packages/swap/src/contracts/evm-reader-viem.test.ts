import type { Log } from "viem";
import { describe, expect, it, vi } from "vitest";
import {
  DEFAULT_EVM_RPCS,
  defaultEvmReaders,
  type EvmLogClient,
  evmReaderFromClient,
} from "./evm-reader-viem.js";

const HTLC = "0xhtlc" as const;
const PH = "0xph" as const;
const CLAIM = "0xclaim" as const;

/** A fake viem client dispatching getLogs by event name. */
function fakeClient(logsByEvent: Record<string, Log[]>): EvmLogClient {
  return {
    getLogs: vi.fn(
      async ({ event }: { event: { name: string } }) =>
        logsByEvent[event.name] ?? [],
    ) as unknown as EvmLogClient["getLogs"],
    getBlock: async () => ({ timestamp: 1_700_000_000n }),
    watchBlocks: ({ onBlock }) => {
      onBlock();
      return () => {};
    },
  };
}

const log = (args: object = {}): Log => ({ args }) as unknown as Log;

describe("evmReaderFromClient", () => {
  it("returns a created event when the swap was funded", async () => {
    const reader = evmReaderFromClient(
      fakeClient({ SwapCreated: [log({ amount: 1000n, token: "0xwbtc" })] }),
    );
    expect(await reader.getHtlcEvents(HTLC, PH, CLAIM)).toEqual([
      { kind: "created", amount: 1000n, token: "0xwbtc" },
    ]);
  });

  it("decodes the revealed preimage from a redeem", async () => {
    const reader = evmReaderFromClient(
      fakeClient({
        SwapCreated: [log({ amount: 1000n, token: "0xwbtc" })],
        SwapRedeemed: [log({ preimage: "0xdeadbeef" })],
      }),
    );
    expect(await reader.getHtlcEvents(HTLC, PH, CLAIM)).toEqual([
      { kind: "created", amount: 1000n, token: "0xwbtc" },
      { kind: "redeemed", preimage: "0xdeadbeef" },
    ]);
  });

  it("reports a refund", async () => {
    const reader = evmReaderFromClient(
      fakeClient({
        SwapCreated: [log({ amount: 1000n, token: "0xwbtc" })],
        SwapRefunded: [log()],
      }),
    );
    expect(await reader.getHtlcEvents(HTLC, PH, CLAIM)).toEqual([
      { kind: "created", amount: 1000n, token: "0xwbtc" },
      { kind: "refunded" },
    ]);
  });

  it("is empty when the HTLC has no events", async () => {
    const reader = evmReaderFromClient(fakeClient({}));
    expect(await reader.getHtlcEvents(HTLC, PH, CLAIM)).toEqual([]);
  });

  it("converts block.timestamp (seconds) to ms", async () => {
    const reader = evmReaderFromClient(fakeClient({}));
    expect(await reader.getBlockTimeMs()).toBe(1_700_000_000_000);
  });

  it("wires watchBlocks through to the callback", async () => {
    const reader = evmReaderFromClient(fakeClient({}));
    const cb = vi.fn();
    reader.watch(cb);
    expect(cb).toHaveBeenCalledTimes(1);
  });
});

describe("defaultEvmReaders", () => {
  it("provides a reader for each chain with tested defaults", () => {
    const readers = defaultEvmReaders();
    expect([...readers.keys()].sort()).toEqual(
      Object.keys(DEFAULT_EVM_RPCS).map(Number).sort(),
    );
  });

  it("keeps the default chains when overriding one", () => {
    const readers = defaultEvmReaders({ 137: "https://my-polygon" });
    expect(readers.has(137)).toBe(true);
    expect(readers.size).toBe(Object.keys(DEFAULT_EVM_RPCS).length);
  });

  it("adds a chain that isn't in the defaults", () => {
    const readers = defaultEvmReaders({ 10: "https://my-optimism" });
    expect(readers.has(10)).toBe(true);
    expect(readers.size).toBe(Object.keys(DEFAULT_EVM_RPCS).length + 1);
  });
});
