/**
 * A viem-backed {@link EvmChainReader} — the concrete chain source the
 * {@link EvmContractManager} uses in production.
 *
 * Kept separate from the manager so the manager itself stays free of any
 * chain-library dependency (and unit-testable against a fake reader). This module
 * is the only place that touches viem and the `HTLCErc20` ABI.
 *
 * All HTLCs on a chain are read with a SINGLE `eth_getLogs` call: the three
 * `HTLCErc20` lifecycle events all index `preimageHash` as topic1, so one filter
 * ORs the three event signatures on topic0 and the tracked preimage hashes on
 * topic1 across all HTLC contract addresses. Request count per scan is constant
 * in the number of tracked swaps — this matters, since public RPCs rate-limit by
 * request count.
 */
import {
  type Block,
  createPublicClient,
  decodeEventLog,
  decodeFunctionResult,
  encodeFunctionData,
  fallback,
  http,
  numberToHex,
  parseAbiItem,
  toEventSelector,
} from "viem";
import type { EvmHtlcEvent } from "./evm.js";
import {
  type EvmActiveQuery,
  type EvmChainReader,
  type EvmHtlcQuery,
  htlcQueryKey,
} from "./evm-manager.js";

/**
 * Tested public RPC endpoints per supported chainId, tried in order via viem's
 * `fallback`. Mirrors the frontend's `evmTransport.ts` list — the client
 * uses these by default so Arkade↔EVM tracking works out of the box; a caller can
 * override per chain with `ClientBuilder.withEvmRpcUrls`.
 */
export const DEFAULT_EVM_RPCS: Record<number, string[]> = {
  // Polygon — viem's default (polygon.drpc.org) misbehaves for some calls, so
  // list working public RPCs explicitly.
  137: [
    "https://polygon.drpc.org",
    "https://tenderly.rpc.polygon.community",
    "https://polygon-bor-rpc.publicnode.com",
  ],
  // No publicnode: it rate-limits by IP across its whole fleet (403s), and as
  // a PRIMARY that costs a full retry cycle per read before failing over.
  // Both entries verified for eth_getLogs + eth_call from a browser origin.
  1: ["https://eth.drpc.org", "https://rpc.mevblocker.io"],
  // The official gateway first; publicnode last (see above).
  42161: [
    "https://arb1.arbitrum.io/rpc",
    "https://arbitrum.drpc.org",
    "https://arbitrum-one-rpc.publicnode.com",
  ],
  // Rootstock: the public node, HTTP only (no WebSocket endpoint exists).
  30: ["https://public-node.rsk.co"],
};

// The three `HTLCErc20` lifecycle events, each indexed by `preimageHash` and each
// carrying `key` — the commitment to the swap's full parameter set, and the only
// field that says which swap an event belongs to.
const SWAP_CREATED = parseAbiItem(
  "event SwapCreated(bytes32 indexed preimageHash, address indexed refundAddress, address indexed claimAddress, address token, uint256 amount, uint256 timelock, bytes32 key)",
);
const SWAP_REDEEMED = parseAbiItem(
  "event SwapRedeemed(bytes32 indexed preimageHash, bytes32 indexed key, bytes32 preimage)",
);
const SWAP_REFUNDED = parseAbiItem(
  "event SwapRefunded(bytes32 indexed preimageHash, bytes32 indexed key)",
);

const HTLC_EVENTS_ABI = [SWAP_CREATED, SWAP_REDEEMED, SWAP_REFUNDED] as const;
/** topic0 of the three lifecycle events — the batched getLogs OR-filter. */
const HTLC_EVENT_TOPICS = HTLC_EVENTS_ABI.map(toEventSelector);

// The contract's open-check: the args hash into the swap key, so `true` with
// the EXPECTED values also verifies the funded terms.
const IS_ACTIVE = parseAbiItem(
  "function isActive(bytes32 preimageHash, uint256 amount, address token, address sender, address claimAddress, uint256 timelock) view returns (bool)",
);

// Multicall3 — same address on every major EVM chain; aggregates the per-HTLC
// isActive calls into one eth_call.
export const MULTICALL3_ADDRESS =
  "0xcA11bde05977b3631167028862bE2a173976CA11" as const;
const AGGREGATE3 = parseAbiItem(
  "function aggregate3((address target, bool allowFailure, bytes callData)[] calls) payable returns ((bool success, bytes returnData)[] returnData)",
);

/** An `EvmActiveQuery` carries the whole tuple, so its key always includes terms. */
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

/** The undecoded log shape `eth_getLogs` returns. */
export type RawLog = {
  address: `0x${string}`;
  topics: [`0x${string}`, ...`0x${string}`[]];
  data: `0x${string}`;
};

/** The viem surface the reader needs — a seam so it can be faked in tests. */
export type EvmLogClient = {
  request(args: {
    method: "eth_getLogs";
    params: [
      {
        address: `0x${string}`[];
        topics: (`0x${string}`[] | null)[];
        // Hex quantity, not "earliest": strict RPCs (e.g. arb1.arbitrum.io)
        // reject the tag on eth_getLogs.
        fromBlock: `0x${string}`;
      },
    ];
  }): Promise<RawLog[]>;
  /** A read-only `eth_call`. */
  call(args: {
    to: `0x${string}`;
    data: `0x${string}`;
  }): Promise<{ data?: `0x${string}` }>;
  getBlock(): Promise<Pick<Block, "timestamp" | "number">>;
};

/** Build an {@link EvmChainReader} over an existing viem-like client. */
export function evmReaderFromClient(client: EvmLogClient): EvmChainReader {
  return {
    async getHtlcEventsBatch(queries, fromBlock = 0n) {
      const results = new Map<string, EvmHtlcEvent[]>();
      if (queries.length === 0) return results;
      for (const q of queries) results.set(htlcQueryKey(q), []);

      const logs = await client.request({
        method: "eth_getLogs",
        params: [
          {
            address: unique(queries.map((q) => q.htlc)),
            topics: [
              HTLC_EVENT_TOPICS,
              unique(queries.map((q) => q.preimageHash)),
            ],
            fromBlock: numberToHex(fromBlock),
          },
        ],
      });

      // A log can only be grouped by what it carries on its own — contract and
      // hash — and several HTLCs can share that. So group, then let every query
      // filter the whole group through its own terms. Matching each log to a
      // single query up front would let two queries sharing a hash collapse into
      // one, dropping the other's settlements or attributing them wrongly.
      const grouped = new Map<string, DecodedHtlcLog[]>();
      for (const log of logs) {
        const decoded = tryDecode(log);
        if (!decoded) continue;
        const group = `${log.address.toLowerCase()}:${decoded.preimageHash.toLowerCase()}`;
        const existing = grouped.get(group);
        if (existing) existing.push(decoded);
        else grouped.set(group, [decoded]);
      }

      for (const query of queries) {
        const group = `${query.htlc.toLowerCase()}:${query.preimageHash.toLowerCase()}`;
        const events = results.get(htlcQueryKey(query));
        if (!events) continue;
        for (const decoded of grouped.get(group) ?? []) {
          const event = toHtlcEvent(decoded, query);
          if (event) events.push(event);
        }
      }
      return results;
    },
    async isActiveBatch(queries) {
      const results = new Map<string, boolean>();
      if (queries.length === 0) return results;
      const calldatas = queries.map((q) =>
        encodeFunctionData({
          abi: [IS_ACTIVE],
          args: [
            q.preimageHash,
            q.amount,
            q.token,
            q.sender,
            q.claimAddress,
            BigInt(q.timelockSec),
          ],
        }),
      );

      const callOne = async (i: number): Promise<void> => {
        const { data } = await client.call({
          to: queries[i].htlc,
          data: calldatas[i],
        });
        results.set(activeKey(queries[i]), decodeIsActive(data));
      };

      if (queries.length === 1) {
        await callOne(0);
        return results;
      }
      try {
        const { data } = await client.call({
          to: MULTICALL3_ADDRESS,
          data: encodeFunctionData({
            abi: [AGGREGATE3],
            args: [
              queries.map((q, i) => ({
                target: q.htlc,
                allowFailure: true,
                callData: calldatas[i],
              })),
            ],
          }),
        });
        if (data === undefined || data === "0x")
          throw new Error("empty multicall result");
        const decoded = decodeFunctionResult({
          abi: [AGGREGATE3],
          data,
        }) as readonly { success: boolean; returnData: `0x${string}` }[];
        queries.forEach((q, i) => {
          const r = decoded[i];
          // A failed inner call reads as "not active" — the caller's log
          // fallback then classifies it, so nothing is silently trusted.
          results.set(
            activeKey(q),
            r?.success === true && decodeIsActive(r.returnData),
          );
        });
      } catch {
        // No Multicall3 on this chain (e.g. a bare dev node) — per-HTLC calls.
        await Promise.all(queries.map((_, i) => callOne(i)));
      }
      return results;
    },
    async getLatestBlock() {
      const block = await client.getBlock();
      return {
        timeMs: Number(block.timestamp) * 1000,
        number: block.number ?? 0n,
      };
    },
  };
}

/** Decode an `isActive` eth_call result; empty data (no contract) → false. */
function decodeIsActive(data: `0x${string}` | undefined): boolean {
  if (data === undefined || data === "0x") return false;
  return decodeFunctionResult({ abi: [IS_ACTIVE], data }) === true;
}

/**
 * The decoded fields this reader acts on.
 *
 * All three events also carry `key`. It is deliberately not surfaced: nothing
 * here can check it against a query yet, and a field decoded but never read
 * reads as though attribution were key-based. See {@link toHtlcEvent}.
 */
type DecodedHtlcLog =
  | {
      eventName: "SwapCreated";
      preimageHash: `0x${string}`;
      claimAddress: `0x${string}`;
      token: `0x${string}`;
      amount: bigint;
    }
  | {
      eventName: "SwapRedeemed";
      preimageHash: `0x${string}`;
      preimage: `0x${string}`;
    }
  | {
      eventName: "SwapRefunded";
      preimageHash: `0x${string}`;
    };

/** Decode one raw log against the three-event ABI; undefined for foreign logs. */
function tryDecode(log: RawLog): DecodedHtlcLog | undefined {
  try {
    const { eventName, args } = decodeEventLog({
      abi: HTLC_EVENTS_ABI,
      data: log.data,
      topics: log.topics,
    });
    return { eventName, ...args } as DecodedHtlcLog;
  } catch {
    return undefined;
  }
}

/**
 * Map a decoded log to the manager's event, applying the per-query guards the
 * batched filter can't express per-hash.
 *
 * Settlements are attributed by `preimageHash` alone, which identifies no single
 * HTLC — any number may share one hash, each with its own terms and lifecycle.
 * Two queries on the same hash and contract therefore both see the same terminal
 * event. Attribution here is ambiguous by construction; `evmObservation`'s
 * `isActive` check, not this mapping, is what keeps an open HTLC from reading as
 * settled, because the tuple it calls with does hold for the leg being tracked.
 *
 * The events carry `key`, the commitment to the full parameter set, which would
 * separate them — but a query cannot arrive at the funded HTLC's key on its own:
 * a coordinator locks its post-swap balance rather than the quoted amount, and is
 * recorded as `sender` in place of the funder named on the swap. Attributing
 * settlements exactly needs the key recorded at funding time to reach the client
 * rather than being re-derived here.
 *
 * `SwapCreated` is guarded on `claimAddress` for the same reason a key match will
 * not do: it is the event that first reports the funded amount, which may differ
 * from the expected one, so its key is not predictable either. `evmObservation`
 * term-checks it afterwards.
 */
function toHtlcEvent(
  decoded: DecodedHtlcLog,
  query: EvmHtlcQuery,
): EvmHtlcEvent | undefined {
  if (decoded.eventName === "SwapCreated") {
    if (decoded.claimAddress.toLowerCase() !== query.claimAddress.toLowerCase())
      return undefined;
    return { kind: "created", amount: decoded.amount, token: decoded.token };
  }

  return decoded.eventName === "SwapRedeemed"
    ? { kind: "redeemed", preimage: decoded.preimage }
    : { kind: "refunded" };
}

function unique<T>(values: T[]): T[] {
  return [...new Set(values)];
}

/**
 * Build an {@link EvmChainReader} over one or more EVM JSON-RPC endpoints. With
 * several, viem's `fallback` tries them in order and fails over on error.
 * Deliberately NOT ranked: ranking health-pings every listed endpoint on an
 * interval from every open tab, which is exactly the background burst that got
 * public RPCs rate-limiting us (publicnode 403s) — and it contradicts this
 * package's near-zero-RPC design.
 */
export function createEvmRpcReader(rpcUrls: string | string[]): EvmChainReader {
  const urls = Array.isArray(rpcUrls) ? rpcUrls : [rpcUrls];
  const transport =
    urls.length > 1 ? fallback(urls.map((url) => http(url))) : http(urls[0]);
  const client = createPublicClient({ transport });
  return evmReaderFromClient(client as unknown as EvmLogClient);
}

/**
 * Resolve the per-chain readers used for tracking: the tested {@link
 * DEFAULT_EVM_RPCS} by default, with any `overrides` taking priority for their
 * chain (kept ahead of the defaults, which remain as fallbacks).
 */
export function defaultEvmReaders(
  overrides?: Record<number, string>,
): Map<number, EvmChainReader> {
  const chainIds = new Set<number>([
    ...Object.keys(DEFAULT_EVM_RPCS).map(Number),
    ...Object.keys(overrides ?? {}).map(Number),
  ]);
  const readers = new Map<number, EvmChainReader>();
  for (const chainId of chainIds) {
    const override = overrides?.[chainId];
    const defaults = DEFAULT_EVM_RPCS[chainId] ?? [];
    const urls = override ? [override, ...defaults] : defaults;
    if (urls.length > 0) readers.set(chainId, createEvmRpcReader(urls));
  }
  return readers;
}
