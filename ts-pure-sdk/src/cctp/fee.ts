/**
 * CCTP v2 fast-transfer fee query against Circle's IRIS API.
 *
 * The fast-transfer fee (`minimumFee`, in basis points × 100) is deducted
 * from the minted amount on the destination chain whenever the burn's
 * `minFinalityThreshold ≤ 1000`. We need to query it to know precisely
 * how much USDC will arrive so the Permit2 signature pulls the right
 * amount and the Multicall doesn't underflow.
 *
 * Mirrors the Rust backend's `swap/src/cctp.rs::compute_max_fee`.
 */
import { FINALITY_FAST, IRIS_API_MAINNET } from "./constants.js";

/** Fee tiers returned by the IRIS API (low/med/high gas estimates for the forwarder). */
export interface IrisForwardFeeTiers {
  low: number;
  med: number;
  high: number;
}

/** Raw entry from `GET /v2/burn/USDC/fees/{sourceDomain}/{destDomain}`. */
export interface IrisFeeEntry {
  finalityThreshold: number;
  /** CCTPv2 fast-transfer fee in basis points (e.g. `1.3` = 0.013%). */
  minimumFee: number;
  /** Forward-service relayer fees (absolute USDC smallest-units). */
  forwardFee: IrisForwardFeeTiers;
}

export interface FetchCctpFeeOptions {
  /** CCTP source domain ID (e.g. 2 = Optimism, 6 = Base). */
  sourceDomain: number;
  /** CCTP destination domain ID (3 = Arbitrum). */
  destinationDomain: number;
  /** Finality threshold to query — defaults to `FINALITY_FAST` (1000). */
  finalityThreshold?: number;
  /** IRIS API base URL. Defaults to mainnet. */
  irisApiUrl?: string;
  /** Optional abort signal. */
  signal?: AbortSignal;
}

/**
 * Fetch the fee entry for the given source → destination pair and
 * finality threshold. Returns the raw IRIS shape — callers that want
 * an applied maxFee should pass this to {@link computeCctpFastFee}.
 */
export async function fetchCctpFee(
  options: FetchCctpFeeOptions,
): Promise<IrisFeeEntry> {
  const {
    sourceDomain,
    destinationDomain,
    finalityThreshold = FINALITY_FAST,
    irisApiUrl = IRIS_API_MAINNET,
    signal,
  } = options;

  const url = `${irisApiUrl}/v2/burn/USDC/fees/${sourceDomain}/${destinationDomain}?forward=true`;
  const response = await fetch(url, { signal });
  if (!response.ok) {
    throw new Error(
      `IRIS fee lookup failed: ${response.status} ${response.statusText}`,
    );
  }
  const entries = (await response.json()) as IrisFeeEntry[];
  const entry =
    entries.find((e) => e.finalityThreshold === finalityThreshold) ??
    entries[0];
  if (!entry) {
    throw new Error(
      `IRIS returned no fee entries for ${sourceDomain}→${destinationDomain}`,
    );
  }
  return entry;
}

/**
 * Tiny TTL cache for CCTP fee lookups. The IRIS fee is effectively
 * constant per-pair minute-to-minute — re-fetching on every quote
 * keystroke would bomb IRIS for no gain. 60s TTL is ample.
 */
const feeCache = new Map<string, { entry: IrisFeeEntry; expiresAt: number }>();
const FEE_CACHE_TTL_MS = 60_000;

/**
 * Cached variant of {@link fetchCctpFee}. Multiple callers within a
 * 60-second window share a single IRIS round-trip per source→dest pair.
 */
export async function getCachedCctpFee(
  options: FetchCctpFeeOptions,
): Promise<IrisFeeEntry> {
  const {
    sourceDomain,
    destinationDomain,
    finalityThreshold = FINALITY_FAST,
    irisApiUrl = IRIS_API_MAINNET,
  } = options;
  const key = `${sourceDomain}-${destinationDomain}-${finalityThreshold}-${irisApiUrl}`;
  const cached = feeCache.get(key);
  if (cached && cached.expiresAt > Date.now()) {
    return cached.entry;
  }
  const entry = await fetchCctpFee(options);
  feeCache.set(key, { entry, expiresAt: Date.now() + FEE_CACHE_TTL_MS });
  return entry;
}

/**
 * Apply IRIS's `minimumFee` (basis points) to an amount in USDC smallest
 * units, matching the Rust backend's formula:
 *   protocolFee = amount × round(minimumFee × 100) ÷ 1_000_000
 *
 * Adds a conservative 20% buffer to absorb rounding drift between quote
 * time and burn time. Return value is the fee in USDC smallest units,
 * suitable for `maxFee` in `depositForBurn` and as the subtracted amount
 * when sizing the Permit2 permit.
 */
export function computeCctpFastFee(
  entry: IrisFeeEntry,
  amountUsdcUnits: bigint,
): bigint {
  const bpsScaled = Math.round(entry.minimumFee * 100);
  const protocolFee = (amountUsdcUnits * BigInt(bpsScaled)) / 1_000_000n;
  return (protocolFee * 120n) / 100n;
}
