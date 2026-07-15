/**
 * A viem-backed {@link EvmChainReader} — the concrete chain source the
 * {@link EvmContractManager} uses in production.
 *
 * Kept separate from the manager so the manager itself stays free of any
 * chain-library dependency (and unit-testable against a fake reader). This module
 * is the only place that touches viem and the `HTLCErc20` ABI.
 */
import {
  type Block,
  createPublicClient,
  fallback,
  http,
  type Log,
  parseAbiItem,
} from "viem";
import type { EvmHtlcEvent } from "./evm.js";
import type { EvmChainReader } from "./evm-manager.js";

/**
 * Tested public RPC endpoints per supported chainId, tried in order via viem's
 * ranked `fallback`. Mirrors the frontend's `evmTransport.ts` list — the client
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
  1: [
    "https://ethereum-rpc.publicnode.com",
    "https://rpc.ankr.com/eth",
    "https://eth.drpc.org",
  ],
  42161: [
    "https://arbitrum-one-rpc.publicnode.com",
    "https://rpc.ankr.com/arbitrum",
    "https://arb1.arbitrum.io/rpc",
  ],
};

// The three `HTLCErc20` lifecycle events, each indexed by `preimageHash`.
const SWAP_CREATED = parseAbiItem(
  "event SwapCreated(bytes32 indexed preimageHash, address indexed refundAddress, address indexed claimAddress, address token, uint256 amount, uint256 timelock)",
);
const SWAP_REDEEMED = parseAbiItem(
  "event SwapRedeemed(bytes32 indexed preimageHash, bytes32 preimage)",
);
const SWAP_REFUNDED = parseAbiItem(
  "event SwapRefunded(bytes32 indexed preimageHash)",
);

/** The viem surface the reader needs — a seam so it can be faked in tests. */
export type EvmLogClient = {
  getLogs(args: {
    address: `0x${string}`;
    event: typeof SWAP_CREATED | typeof SWAP_REDEEMED | typeof SWAP_REFUNDED;
    args: { preimageHash: `0x${string}`; claimAddress?: `0x${string}` };
    fromBlock: bigint | "earliest";
  }): Promise<Log[]>;
  getBlock(): Promise<Pick<Block, "timestamp">>;
  watchBlocks(args: { onBlock: () => void }): () => void;
};

/** Build an {@link EvmChainReader} over an existing viem-like client. */
export function evmReaderFromClient(client: EvmLogClient): EvmChainReader {
  return {
    async getHtlcEvents(htlc, preimageHash, claimAddress) {
      const [created, redeemed, refunded] = await Promise.all([
        // Filter by claimAddress too (also indexed), so only the HTLC actually
        // claimable on the swap's terms is seen.
        client.getLogs({
          address: htlc,
          event: SWAP_CREATED,
          args: { preimageHash, claimAddress },
          fromBlock: "earliest",
        }),
        client.getLogs({
          address: htlc,
          event: SWAP_REDEEMED,
          args: { preimageHash },
          fromBlock: "earliest",
        }),
        client.getLogs({
          address: htlc,
          event: SWAP_REFUNDED,
          args: { preimageHash },
          fromBlock: "earliest",
        }),
      ]);
      // Order doesn't matter — evmObservation resolves precedence.
      const events: EvmHtlcEvent[] = [];
      for (const log of created) {
        const args = (
          log as { args?: { amount?: bigint; token?: `0x${string}` } }
        ).args;
        events.push({
          kind: "created",
          amount: args?.amount ?? 0n,
          token: args?.token ?? "0x",
        });
      }
      for (const log of redeemed) {
        const preimage = (log as { args?: { preimage?: `0x${string}` } }).args
          ?.preimage;
        if (preimage) events.push({ kind: "redeemed", preimage });
      }
      if (refunded.length > 0) events.push({ kind: "refunded" });
      return events;
    },
    async getBlockTimeMs() {
      const block = await client.getBlock();
      return Number(block.timestamp) * 1000;
    },
    watch(cb) {
      return client.watchBlocks({ onBlock: () => cb() });
    },
  };
}

/**
 * Build an {@link EvmChainReader} over one or more EVM JSON-RPC endpoints. With
 * several, viem's ranked `fallback` picks the healthiest and fails over.
 */
export function createEvmRpcReader(rpcUrls: string | string[]): EvmChainReader {
  const urls = Array.isArray(rpcUrls) ? rpcUrls : [rpcUrls];
  const transport =
    urls.length > 1
      ? fallback(
          urls.map((url) => http(url)),
          { rank: { interval: 60_000, sampleCount: 3, timeout: 1_000 } },
        )
      : http(urls[0]);
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
