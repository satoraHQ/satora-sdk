import {
  encodeAbiParameters,
  encodeEventTopics,
  encodeFunctionResult,
  keccak256,
  numberToHex,
  parseAbiItem,
} from "viem";
import { describe, expect, it, vi } from "vitest";
import { htlcQueryKey } from "./evm-manager.js";
import {
  DEFAULT_EVM_RPCS,
  defaultEvmReaders,
  type EvmLogClient,
  evmReaderFromClient,
  MULTICALL3_ADDRESS,
  type RawLog,
} from "./evm-reader-viem.js";

const HTLC = `0x${"11".repeat(20)}` as const;
const PH = `0x${"22".repeat(32)}` as const;
const CLAIM = `0x${"33".repeat(20)}` as const;
const REFUND = `0x${"44".repeat(20)}` as const;
const TOKEN = `0x${"55".repeat(20)}` as const;

/** The rest of the swap-key tuple for {@link QUERY}, and the key it hashes to. */
const TERMS = {
  amount: 1000n,
  token: TOKEN,
  sender: REFUND,
  timelockSec: 1_700_000_000,
} as const;
const SWAP_KEY = keccak256(
  encodeAbiParameters(
    [
      { type: "bytes32" },
      { type: "uint256" },
      { type: "address" },
      { type: "address" },
      { type: "address" },
      { type: "uint256" },
    ],
    [
      PH,
      TERMS.amount,
      TERMS.token,
      TERMS.sender,
      CLAIM,
      BigInt(TERMS.timelockSec),
    ],
  ),
);
/** A swap sharing PH but locked on different terms, so a different key. */
const OTHER_KEY = `0x${"ee".repeat(32)}` as const;

const CREATED = parseAbiItem(
  "event SwapCreated(bytes32 indexed preimageHash, address indexed refundAddress, address indexed claimAddress, address token, uint256 amount, uint256 timelock, bytes32 key)",
);
const REDEEMED = parseAbiItem(
  "event SwapRedeemed(bytes32 indexed preimageHash, bytes32 indexed key, bytes32 preimage)",
);
const REFUNDED = parseAbiItem(
  "event SwapRefunded(bytes32 indexed preimageHash, bytes32 indexed key)",
);

/** Raw `eth_getLogs`-shaped logs, properly ABI-encoded so decoding is real. */
function createdLog(
  amount: bigint,
  claimAddress: `0x${string}` = CLAIM,
): RawLog {
  return {
    address: HTLC,
    topics: encodeEventTopics({
      abi: [CREATED],
      args: { preimageHash: PH, refundAddress: REFUND, claimAddress },
    }) as RawLog["topics"],
    data: encodeAbiParameters(
      [
        { type: "address" },
        { type: "uint256" },
        { type: "uint256" },
        { type: "bytes32" },
      ],
      [TOKEN, amount, 0n, SWAP_KEY],
    ),
  };
}

function redeemedLog(
  preimage: `0x${string}`,
  key: `0x${string}` = SWAP_KEY,
): RawLog {
  return {
    address: HTLC,
    topics: encodeEventTopics({
      abi: [REDEEMED],
      args: { preimageHash: PH, key },
    }) as RawLog["topics"],
    data: encodeAbiParameters([{ type: "bytes32" }], [preimage]),
  };
}

function refundedLog(key: `0x${string}` = SWAP_KEY): RawLog {
  return {
    address: HTLC,
    topics: encodeEventTopics({
      abi: [REFUNDED],
      args: { preimageHash: PH, key },
    }) as RawLog["topics"],
    data: "0x",
  };
}

function fakeClient(logs: RawLog[]): EvmLogClient & {
  request: ReturnType<typeof vi.fn>;
  call: ReturnType<typeof vi.fn>;
} {
  return {
    request: vi.fn(async () => logs),
    call: vi.fn(async () => ({ data: "0x" as const })),
    getBlock: async () => ({ timestamp: 1_700_000_000n, number: 42n }),
  };
}

const IS_ACTIVE = parseAbiItem(
  "function isActive(bytes32 preimageHash, uint256 amount, address token, address sender, address claimAddress, uint256 timelock) view returns (bool)",
);
const AGGREGATE3 = parseAbiItem(
  "function aggregate3((address target, bool allowFailure, bytes callData)[] calls) payable returns ((bool success, bytes returnData)[] returnData)",
);

const boolResult = (value: boolean): `0x${string}` =>
  encodeFunctionResult({ abi: [IS_ACTIVE], result: value });

const activeQueryFor = (htlc: `0x${string}`, preimageHash: `0x${string}`) => ({
  htlc,
  preimageHash,
  amount: 1000n,
  token: TOKEN,
  sender: REFUND,
  claimAddress: CLAIM,
  timelockSec: 1_700_000_000,
});

/** The key an `isActiveBatch` result carries: the whole tuple is always known. */
const activeKeyFor = (htlc: `0x${string}`, preimageHash: `0x${string}`) =>
  htlcQueryKey({
    htlc,
    preimageHash,
    claimAddress: CLAIM,
    terms: {
      amount: 1000n,
      token: TOKEN,
      sender: REFUND,
      timelockSec: 1_700_000_000,
    },
  });

const QUERY = { htlc: HTLC, preimageHash: PH, claimAddress: CLAIM };
/** The same swap, with enough terms to derive its key. */
const QUERY_WITH_TERMS = { ...QUERY, terms: TERMS };
const KEY = htlcQueryKey(QUERY);
/** The result key when the query carries terms — a different key by design. */
const KEY_WITH_TERMS = htlcQueryKey(QUERY_WITH_TERMS);

describe("evmReaderFromClient", () => {
  it("returns a created event when the swap was funded", async () => {
    const reader = evmReaderFromClient(fakeClient([createdLog(1000n)]));
    const events = (await reader.getHtlcEventsBatch([QUERY])).get(KEY);
    expect(events).toHaveLength(1);
    expect(events?.[0]).toMatchObject({ kind: "created", amount: 1000n });
    expect((events?.[0] as { token: string }).token.toLowerCase()).toBe(TOKEN);
  });

  it("attributes a settlement by swap key when the terms are known", async () => {
    const reader = evmReaderFromClient(
      fakeClient([redeemedLog(`0x${"ab".repeat(32)}`)]),
    );
    const events = (await reader.getHtlcEventsBatch([QUERY_WITH_TERMS])).get(
      KEY_WITH_TERMS,
    );
    expect(events).toEqual([
      { kind: "redeemed", preimage: `0x${"ab".repeat(32)}` },
    ]);
  });

  it("takes a settlement on the hash the filter matched", async () => {
    // A settlement emitted by any HTLC under this hash is reported, including one
    // that is not this swap's. The swap's own tuple cannot derive the funded
    // HTLC's key — a coordinator locks its post-swap balance rather than the
    // quoted amount, and the contract records the coordinator as sender, not the
    // funder named on the swap — so a key match here would discard this swap's
    // real settlements. `isActive`, whose tuple does hold for a server-funded
    // leg, is what stops an open HTLC being read as settled.
    const reader = evmReaderFromClient(
      fakeClient([
        redeemedLog(`0x${"ab".repeat(32)}`, OTHER_KEY),
        refundedLog(OTHER_KEY),
      ]),
    );
    expect(
      (await reader.getHtlcEventsBatch([QUERY_WITH_TERMS])).get(KEY_WITH_TERMS),
    ).toEqual([
      { kind: "redeemed", preimage: `0x${"ab".repeat(32)}` },
      { kind: "refunded" },
    ]);
  });

  it("reports a settlement whether or not the query carries terms", async () => {
    const reader = evmReaderFromClient(fakeClient([refundedLog()]));
    expect((await reader.getHtlcEventsBatch([QUERY])).get(KEY)).toEqual([
      { kind: "refunded" },
    ]);
  });

  it("gives two queries sharing a hash their own result entries", async () => {
    // They see the same logs — a settlement is taken on the hash — but each holds
    // its own entry, so neither overwrites the other in the batch result.
    const otherQuery = { ...QUERY, terms: { ...TERMS, amount: 999n } };

    const reader = evmReaderFromClient(fakeClient([refundedLog()]));
    const result = await reader.getHtlcEventsBatch([
      QUERY_WITH_TERMS,
      otherQuery,
    ]);

    expect(htlcQueryKey(QUERY_WITH_TERMS)).not.toBe(htlcQueryKey(otherQuery));
    expect(result.get(htlcQueryKey(QUERY_WITH_TERMS))).toEqual([
      { kind: "refunded" },
    ]);
    expect(result.get(htlcQueryKey(otherQuery))).toEqual([
      { kind: "refunded" },
    ]);
  });

  it("drops a SwapCreated paying a different claim address", async () => {
    const other = `0x${"66".repeat(20)}` as const;
    const reader = evmReaderFromClient(fakeClient([createdLog(1000n, other)]));
    expect((await reader.getHtlcEventsBatch([QUERY])).get(KEY)).toEqual([]);
  });

  it("decodes the revealed preimage from a redeem", async () => {
    const preimage = `0x${"ab".repeat(32)}` as const;
    const reader = evmReaderFromClient(
      fakeClient([createdLog(1000n), redeemedLog(preimage)]),
    );
    expect(
      (await reader.getHtlcEventsBatch([QUERY_WITH_TERMS])).get(KEY_WITH_TERMS),
    ).toMatchObject([
      { kind: "created", amount: 1000n },
      { kind: "redeemed", preimage },
    ]);
  });

  it("reports a refund", async () => {
    const reader = evmReaderFromClient(
      fakeClient([createdLog(1000n), refundedLog()]),
    );
    expect(
      (await reader.getHtlcEventsBatch([QUERY_WITH_TERMS])).get(KEY_WITH_TERMS),
    ).toMatchObject([{ kind: "created", amount: 1000n }, { kind: "refunded" }]);
  });

  it("maps a queried HTLC with no events to an empty array", async () => {
    const reader = evmReaderFromClient(fakeClient([]));
    expect((await reader.getHtlcEventsBatch([QUERY])).get(KEY)).toEqual([]);
  });

  it("makes no request at all for an empty batch", async () => {
    const client = fakeClient([]);
    const reader = evmReaderFromClient(client);
    expect((await reader.getHtlcEventsBatch([])).size).toBe(0);
    expect(client.request).not.toHaveBeenCalled();
  });

  it("issues ONE eth_getLogs for many HTLCs, ORing addresses and hashes", async () => {
    const ph2 = `0x${"77".repeat(32)}` as const;
    const htlc2 = `0x${"88".repeat(20)}` as const;
    const client = fakeClient([createdLog(1000n)]);
    const reader = evmReaderFromClient(client);

    const result = await reader.getHtlcEventsBatch([
      QUERY,
      { htlc: htlc2, preimageHash: ph2, claimAddress: CLAIM },
    ]);

    expect(client.request).toHaveBeenCalledTimes(1);
    const filter = client.request.mock.calls[0][0].params[0];
    expect(filter.address).toEqual([HTLC, htlc2]);
    expect(filter.topics[1]).toEqual([PH, ph2]);
    expect(filter.fromBlock).toBe("0x0");
    // The log only matches the first query; the second maps to empty.
    expect(result.get(KEY)).toHaveLength(1);
    expect(
      result.get(htlcQueryKey({ htlc: htlc2, preimageHash: ph2 })),
    ).toEqual([]);
  });

  it("skips a log it cannot decode instead of failing the batch", async () => {
    const junk: RawLog = {
      address: HTLC,
      topics: [`0x${"99".repeat(32)}`],
      data: "0x",
    };
    const reader = evmReaderFromClient(fakeClient([junk, createdLog(1000n)]));
    expect((await reader.getHtlcEventsBatch([QUERY])).get(KEY)).toHaveLength(1);
  });

  it("passes fromBlock through as a hex quantity", async () => {
    const client = fakeClient([]);
    const reader = evmReaderFromClient(client);
    await reader.getHtlcEventsBatch([QUERY], 123n);
    expect(client.request.mock.calls[0][0].params[0].fromBlock).toBe("0x7b");
  });

  it("reports the latest block time (ms) and number", async () => {
    const reader = evmReaderFromClient(fakeClient([]));
    expect(await reader.getLatestBlock()).toEqual({
      timeMs: 1_700_000_000_000,
      number: 42n,
    });
  });
});

describe("isActiveBatch", () => {
  it("a single query goes straight to the HTLC contract", async () => {
    const client = fakeClient([]);
    client.call.mockResolvedValueOnce({ data: boolResult(true) });
    const reader = evmReaderFromClient(client);

    const result = await reader.isActiveBatch([activeQueryFor(HTLC, PH)]);

    expect(client.call).toHaveBeenCalledTimes(1);
    expect(client.call.mock.calls[0][0].to).toBe(HTLC);
    expect(result.get(activeKeyFor(HTLC, PH))).toBe(true);
  });

  it("many queries collapse into one Multicall3 eth_call", async () => {
    const htlc2 = `0x${"88".repeat(20)}` as const;
    const ph2 = `0x${"77".repeat(32)}` as const;
    const client = fakeClient([]);
    client.call.mockResolvedValueOnce({
      data: encodeFunctionResult({
        abi: [AGGREGATE3],
        result: [
          { success: true, returnData: boolResult(true) },
          { success: false, returnData: "0x" },
        ],
      }),
    });
    const reader = evmReaderFromClient(client);

    const result = await reader.isActiveBatch([
      activeQueryFor(HTLC, PH),
      activeQueryFor(htlc2, ph2),
    ]);

    expect(client.call).toHaveBeenCalledTimes(1);
    expect(client.call.mock.calls[0][0].to).toBe(MULTICALL3_ADDRESS);
    expect(result.get(activeKeyFor(HTLC, PH))).toBe(true);
    // A failed inner call reads as inactive (the caller's log path classifies).
    expect(result.get(activeKeyFor(htlc2, ph2))).toBe(false);
  });

  it("falls back to per-HTLC calls when Multicall3 is unavailable", async () => {
    const htlc2 = `0x${"88".repeat(20)}` as const;
    const ph2 = `0x${"77".repeat(32)}` as const;
    const client = fakeClient([]);
    client.call
      .mockResolvedValueOnce({ data: "0x" }) // multicall address is empty → decode fails
      .mockResolvedValueOnce({ data: boolResult(true) })
      .mockResolvedValueOnce({ data: boolResult(false) });
    const reader = evmReaderFromClient(client);

    const result = await reader.isActiveBatch([
      activeQueryFor(HTLC, PH),
      activeQueryFor(htlc2, ph2),
    ]);

    expect(client.call).toHaveBeenCalledTimes(3);
    expect(result.get(activeKeyFor(HTLC, PH))).toBe(true);
    expect(result.get(activeKeyFor(htlc2, ph2))).toBe(false);
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

describe("chunked eth_getLogs", () => {
  it("splits a read past the provider's span cap into consecutive chunks", async () => {
    // The redeem sits in the last chunk; the merged result must still carry it.
    const request = vi.fn(
      async (args: { params: [{ fromBlock: string; toBlock?: string }] }) =>
        args.params[0].toBlock === numberToHex(5000n)
          ? [createdLog(1n), redeemedLog(`0x${"ab".repeat(32)}`)]
          : [],
    );
    const client: EvmLogClient = {
      request: request as unknown as EvmLogClient["request"],
      call: async () => ({ data: "0x" as const }),
      getBlock: async () => ({ timestamp: 1_700_000_000n, number: 5000n }),
    };
    const reader = evmReaderFromClient(client, { maxBlockRange: 2000n });
    const query = {
      htlc: HTLC,
      preimageHash: PH,
      claimAddress: CLAIM,
      terms: TERMS,
    };
    const events = await reader.getHtlcEventsBatch([query], 0n);

    const ranges = request.mock.calls.map(([args]) => [
      args.params[0].fromBlock,
      args.params[0].toBlock,
    ]);
    expect(ranges).toEqual([
      [numberToHex(0n), numberToHex(1999n)],
      [numberToHex(2000n), numberToHex(3999n)],
      [numberToHex(4000n), numberToHex(5000n)],
    ]);
    expect(events.get(htlcQueryKey(query))?.map((e) => e.kind)).toEqual([
      "created",
      "redeemed",
    ]);
  });

  it("makes one unbounded call without a cap", async () => {
    const client = fakeClient([]);
    const reader = evmReaderFromClient(client);
    await reader.getHtlcEventsBatch(
      [{ htlc: HTLC, preimageHash: PH, claimAddress: CLAIM, terms: TERMS }],
      7n,
    );
    expect(client.request).toHaveBeenCalledOnce();
    expect(client.request.mock.calls[0][0].params[0].toBlock).toBeUndefined();
  });
});
