/**
 * `composeQuote` — build an end-to-end `QuoteResponse` from the three
 * server primitives:
 *
 *  - `/swap-pairs`    — static rates and limits per pair.
 *  - `/network-fees`  — current gas/mining sats per pair.
 *  - `/dex-quote`     — live DEX-leg pricing on the settlement hub.
 *
 * The output is the *same* `QuoteResponse` shape as `getQuote()` so the
 * two are substitutable. An E2E parity harness keeps them numerically
 * aligned; once parity holds across all supported pairs, the legacy
 * `getQuote()` body gets replaced with a call to this and the names
 * swap.
 *
 * # Supported pairs
 *
 *  - A BTC-denominated side (`source_token`/`target_token` = `"btc"`) on
 *    Bitcoin, Arkade, or Lightning, paired with an EVM token on any chain
 *    present in `/chain-config` (Arbitrum, Polygon, …).
 *  - Either direction (BTC-side → EVM, or EVM → BTC-side).
 *  - Either `sourceAmount` or `targetAmount` pinned.
 *  - Direct BTC↔pegged-pivot targets (tBTC / WBTC) skip the DEX; other
 *    targets route through `/dex-quote`.
 *
 * Foreign-target EVM↔EVM (neither side BTC) is not supported and throws
 * `UnsupportedComposeQuotePath`.
 */

import type {
  Chain,
  ChainConfigResponse,
  NetworkFeesResponse,
  QuoteResponse,
  SwapPairsResponse,
  Token,
} from "./types/index.js";

/**
 * Look up the BTC-pegged pivot token for an EVM chain.
 *
 * The truth lives on the server (`config.wbtc_address(chain)` + the
 * token list for decimals/symbol) and ships in `/chain-config`. Pulling
 * it from the response — rather than hardcoding a per-chain map in the
 * SDK — means new chains light up the moment the server's config does.
 *
 * Both `composeBtcToEvm` and `composeEvmToBtc` compute the scale as
 * `10^(decimals - 8)`, so any chain whose `btc_pegged_token` is in the
 * range [8, 18] decimals works without further changes.
 */
interface Pivot {
  address: string;
  decimals: number;
  /** Pegged-token units per 1 BTC (`"1"` for tBTC, WBTC/BTC rate for WBTC). */
  pegRate: string;
}

function pivotForChain(
  chainConfig: ChainConfigResponse,
  chain: Chain,
): Pivot | undefined {
  const entry = chainConfig.chains.find((c) => c.chain === chain);
  if (!entry) return undefined;
  return {
    address: entry.btc_pegged_token.address,
    decimals: entry.btc_pegged_token.decimals,
    pegRate: entry.btc_peg_rate,
  };
}

/** Arbitrum — the settlement hub every bridge pivots through. */
const ARBITRUM_CHAIN = "42161" as Chain;
const ARBITRUM_CHAIN_ID = 42161;

/** How the EVM leg reaches a given chain — directly on a hub, or bridged. */
interface EvmRouting {
  /** Pivot token on the settlement chain: the EVM chain itself if it's a hub,
   *  otherwise Arbitrum's tBTC. */
  btcPegged: Pivot;
  /** Chain id the pivot + HTLC live on (the DEX pivot side). */
  pivotChainId: number;
  /** Chain used for swap-pair / network-fee lookups (the settlement hub). */
  hubChain: Chain;
  /** Chain id of the user's EVM token (same as `pivotChainId` when direct,
   *  the remote chain when bridged). */
  tokenChainId: number;
  /** True when the route bridges through Arbitrum. */
  bridged: boolean;
}

/**
 * Decide how the EVM leg reaches `evmChain`:
 *
 * - **hub** (a chain with a BTC-pegged pivot in `/chain-config` — today
 *   Arbitrum, Polygon, Ethereum) → swap there directly; pivot/HTLC and the
 *   user's token share the chain, no bridge.
 * - **anything else** (Optimism, Base, Solana, USDT0-only chains, …) → bridge
 *   through Arbitrum: the HTLC settles in tBTC on Arbitrum, the DEX leg crosses
 *   Arbitrum↔`evmChain`, and the swap-pair / network-fee legs key off Arbitrum.
 *   The bridge protocol (CCTP vs OFT) and its fee are resolved server-side from
 *   the token, and `/dex-quote` already folds the fee into the amounts.
 *
 * Note we don't bridge a token that *is* directly swappable on a hub even when
 * that hub also happens to be a bridge chain (e.g. USDC@Polygon): Polygon is a
 * hub, so it routes direct and skips Circle's fee entirely.
 */
function resolveEvmRouting(
  chainConfig: ChainConfigResponse,
  evmChain: Chain,
): EvmRouting {
  const tokenChainId = Number.parseInt(evmChain, 10);
  if (Number.isNaN(tokenChainId)) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: EVM chain ${evmChain} is not numeric`,
    );
  }
  const direct = pivotForChain(chainConfig, evmChain);
  if (direct) {
    return {
      btcPegged: direct,
      pivotChainId: tokenChainId,
      hubChain: evmChain,
      tokenChainId,
      bridged: false,
    };
  }
  const arb = pivotForChain(chainConfig, ARBITRUM_CHAIN);
  if (!arb) {
    throw new UnsupportedComposeQuotePath(
      "composeQuote: Arbitrum pivot missing from chain-config; cannot bridge",
    );
  }
  return {
    btcPegged: arb,
    pivotChainId: ARBITRUM_CHAIN_ID,
    hubChain: ARBITRUM_CHAIN,
    tokenChainId,
    bridged: true,
  };
}

/**
 * Parse a decimal rate string ("1", "0.99790000") into an exact
 * numerator/denominator so the BTC↔pegged conversion is integer-exact
 * and matches the server's `rust_decimal` arithmetic to the unit.
 */
function parseRate(rate: string): { num: bigint; den: bigint } {
  const [intPart, fracPart = ""] = rate.split(".");
  const num = BigInt(`${intPart}${fracPart}` || "0");
  const den = 10n ** BigInt(fracPart.length);
  return { num, den };
}

// Direct BTC↔pegged conversions, applying the pivot's `btc_peg_rate` as a
// *consistent* exchange rate: `pegged = sats × rate` forward,
// `sats = pegged / rate` inverse — a true inverse, so a round trip is
// lossless. The server currently reports `btc_peg_rate = "1"` for every
// chain (BTC pegs treated 1:1), so these reduce to pure decimal scaling;
// the rate is plumbed through for the day a chain's peg isn't 1:1. (Note:
// this deliberately does NOT reproduce legacy `calculate_wbtc_amounts`'s
// symmetric ×rate haircut on WBTC — see the composed-only WBTC tests.)

/** Pegged base units = round(btcSats × rate) × pivotScale. */
function satsToPeggedBase(
  btcSats: bigint,
  pegRate: string,
  pivotScale: bigint,
): bigint {
  const { num, den } = parseRate(pegRate);
  // round(btcSats × num / den), half away from zero (positive amounts).
  const peggedSats = (btcSats * num + den / 2n) / den;
  return peggedSats * pivotScale;
}

/** BTC sats = trunc((peggedBase / pivotScale) / rate). */
function peggedBaseToSats(
  peggedBase: bigint,
  pegRate: string,
  pivotScale: bigint,
): bigint {
  const { num, den } = parseRate(pegRate);
  // trunc((peggedBase / pivotScale) / (num / den)) = peggedBase × den / (pivotScale × num)
  return (peggedBase * den) / (pivotScale * num);
}

/** True if `token` is the chain's BTC-pegged pivot (case-insensitive). */
function isPivotToken(token: string, btcPegged: { address: string }): boolean {
  return token.toLowerCase() === btcPegged.address.toLowerCase();
}

/**
 * Direct BTC → pegged-token pivot, no DEX leg — the target token *is* the
 * pivot (e.g. BTC → tBTC). The BTC↔pegged conversion uses the pivot's
 * `pegRate` from `/chain-config`: exact 1:1 for tBTC, the live WBTC/BTC
 * rate for WBTC. Matches the server's `calculate_wbtc_amounts` to the
 * unit (round-half-up forward, truncating inverse).
 */
function directBtcToPeggedPivot(
  params: ComposeQuoteParams,
  pivot: Pivot,
  pivotScale: bigint,
): BtcEvmPivot {
  let btcSats: bigint;
  let evmSmallest: bigint;
  if (params.sourceAmount != null) {
    // Source-pinned: BTC sats in, pegged base units out.
    btcSats = BigInt(params.sourceAmount);
    evmSmallest = satsToPeggedBase(btcSats, pivot.pegRate, pivotScale);
  } else if (params.targetAmount != null) {
    // Target-pinned: pegged base units in, BTC sats out.
    evmSmallest = BigInt(params.targetAmount);
    btcSats = peggedBaseToSats(evmSmallest, pivot.pegRate, pivotScale);
  } else {
    throw new UnsupportedComposeQuotePath(
      "composeQuote: no amount pinned (unreachable)",
    );
  }
  return { btcSats, evmSmallest, evmDecimals: pivot.decimals, bridgeFee: 0n };
}

/**
 * Direct pegged-token → BTC pivot, no DEX leg — the source token *is* the
 * pivot (e.g. tBTC → BTC). Mirror of {@link directBtcToPeggedPivot}.
 */
function directPeggedToBtcPivot(
  params: ComposeQuoteParams,
  pivot: Pivot,
  pivotScale: bigint,
): EvmBtcPivot {
  let btcSats: bigint;
  let evmSmallest: bigint;
  if (params.sourceAmount != null) {
    // Source-pinned: pegged base units in, BTC sats out.
    evmSmallest = BigInt(params.sourceAmount);
    btcSats = peggedBaseToSats(evmSmallest, pivot.pegRate, pivotScale);
  } else if (params.targetAmount != null) {
    // Target-pinned: BTC sats in, pegged base units out.
    btcSats = BigInt(params.targetAmount);
    evmSmallest = satsToPeggedBase(btcSats, pivot.pegRate, pivotScale);
  } else {
    throw new UnsupportedComposeQuotePath(
      "composeQuote: no amount pinned (unreachable)",
    );
  }
  return { btcSats, evmSmallest, evmDecimals: pivot.decimals, bridgeFee: 0n };
}

export class UnsupportedComposeQuotePath extends Error {
  constructor(message: string) {
    super(message);
    this.name = "UnsupportedComposeQuotePath";
  }
}

export interface ComposeQuoteParams {
  sourceChain: Chain;
  sourceToken: string;
  targetChain: Chain;
  targetToken: string;
  sourceAmount?: number;
  targetAmount?: number;
  slippageBps?: number;
}

/**
 * Inputs `composeQuote` needs from the rest of the SDK. Passed in as a
 * dependency object rather than baked into the function so callers can
 * cache `/swap-pairs` and `/network-fees` across many quote attempts.
 *
 * In the SDK's `Client.composeQuote()` wrapper these are fetched fresh
 * each call; in tighter UI loops a caller can pre-fetch them and reuse.
 */
export interface ComposeQuoteDeps {
  swapPairs: SwapPairsResponse;
  networkFees: NetworkFeesResponse;
  chainConfig: ChainConfigResponse;

  /**
   * DEX-leg quote primitive — `Client.getDexQuote` once bound to the
   * underlying HTTP client.
   */
  getDexQuote(params: {
    from: Token;
    to: Token;
    amount: { kind: "exact_in" | "exact_out"; value: string };
    slippageBps: number;
  }): Promise<{
    expected_amount_in: { raw: string; decimals: number };
    estimated_amount_out: { raw: string; decimals: number };
    /** Bridge fee deducted (bridged-token smallest units); absent if no bridge. */
    bridge_fee?: string;
  }>;
}

export async function composeQuote(
  params: ComposeQuoteParams,
  deps: ComposeQuoteDeps,
): Promise<QuoteResponse> {
  const sourceIsBtc = params.sourceToken.toLowerCase() === "btc";
  const targetIsBtc = params.targetToken.toLowerCase() === "btc";

  if (sourceIsBtc === targetIsBtc) {
    throw new UnsupportedComposeQuotePath(
      "composeQuote currently requires exactly one BTC side; foreign-target EVM↔EVM not implemented yet",
    );
  }
  const sourcePinned = params.sourceAmount != null;
  const targetPinned = params.targetAmount != null;
  if (sourcePinned === targetPinned) {
    throw new UnsupportedComposeQuotePath(
      "specify exactly one of sourceAmount or targetAmount",
    );
  }

  if (sourceIsBtc) {
    return composeBtcToEvm(params, deps);
  }
  return composeEvmToBtc(params, deps);
}

async function composeBtcToEvm(
  params: ComposeQuoteParams,
  deps: ComposeQuoteDeps,
): Promise<QuoteResponse> {
  // Direct on a hub chain, or bridged through Arbitrum for everything else.
  const { btcPegged, pivotChainId, hubChain, tokenChainId } = resolveEvmRouting(
    deps.chainConfig,
    params.targetChain,
  );

  // The BTC↔pivot leg (rate, fee, limits) keys off the settlement hub —
  // Arbitrum for a bridged target, the target chain itself when direct.
  const pair = lookupPair(deps.swapPairs, params.sourceChain, hubChain);
  const feeEntry = lookupFeeEntry(
    deps.networkFees,
    params.sourceChain,
    hubChain,
  );
  const networkFeeSats = feeEntry.fees.source_sats + feeEntry.fees.target_sats;
  // `gasless_network_fee` is always 0: the gasless settlement cost is folded
  // into `network_fee` upstream (the `/network-fees` values already include it
  // on chains that pass settlement gas through). The field is kept on the
  // QuoteResponse for wire compatibility with `getQuote()`.
  const gaslessNetworkFee = 0;

  // Pivot scale: BTC sats (8-dec) ↔ tBTC/WBTC base units. Direction-agnostic
  // — same factor used to inflate (source-pinned) or deflate (target-pinned).
  const pivotScale = 10n ** BigInt(btcPegged.decimals - 8);

  // Run the DEX leg in whichever direction the caller pinned. Both paths
  // produce the same shape: (btc_sats, evm_smallest), with the DEX quote
  // carrying the precise opposite side. The entry-point guarantees exactly
  // one side is pinned; the trailing throw is unreachable but keeps the
  // amount typed as a non-null `bigint` for each branch.
  let pivot: BtcEvmPivot;
  if (isPivotToken(params.targetToken, btcPegged)) {
    // Direct BTC → pegged token (e.g. BTC → tBTC): the pegged token *is*
    // the pivot, so there's no DEX hop — just the 1:1 BTC peg. See
    // directBtcToPeggedPivot for the tBTC-exact / WBTC caveat.
    pivot = directBtcToPeggedPivot(params, btcPegged, pivotScale);
  } else if (params.sourceAmount != null) {
    pivot = await dexSourcePinned(
      BigInt(params.sourceAmount),
      params,
      deps,
      btcPegged,
      pivotChainId,
      tokenChainId,
      pivotScale,
    );
  } else if (params.targetAmount != null) {
    pivot = await dexTargetPinned(
      BigInt(params.targetAmount),
      params,
      deps,
      btcPegged,
      pivotChainId,
      tokenChainId,
      pivotScale,
    );
  } else {
    throw new UnsupportedComposeQuotePath(
      "composeQuote: no amount pinned (unreachable)",
    );
  }

  const { btcSats, evmSmallest, evmDecimals, bridgeFee } = pivot;

  // Protocol fee in sats: floor(btc_sats × fee_percentage). The server
  // uses `rust_decimal` arithmetic on a `Decimal`; we use `BigInt`
  // multiplication after scaling the percentage by 1e18 to dodge
  // floating-point drift.
  const FEE_SCALE = 10n ** 18n;
  const feePctScaled = BigInt(
    Math.round(pair.fee_percentage * Number(FEE_SCALE)),
  );
  const protocolFee = Number((btcSats * feePctScaled) / FEE_SCALE);
  const totalFeeSats = networkFeeSats + gaslessNetworkFee + protocolFee;

  // Net amounts mirror quote_calculator::compute_net_amounts for the BTC→EVM
  // arms. Source-pinned: net_target = sats_to_evm(btc_sats − fees).
  // Target-pinned: net_source = btc_sats + fees, target echoes pinned.
  let netSource: bigint;
  let netTarget: bigint;
  if (params.sourceAmount != null) {
    const effectiveSats = btcSats - BigInt(totalFeeSats);
    netSource = btcSats;
    netTarget =
      btcSats === 0n
        ? 0n
        : (effectiveSats < 0n ? 0n : effectiveSats * evmSmallest) / btcSats;
  } else {
    netSource = btcSats + BigInt(totalFeeSats);
    netTarget = evmSmallest;
  }

  return {
    exchange_rate: calculateExchangeRate(btcSats, evmSmallest, evmDecimals),
    network_fee: networkFeeSats,
    gasless_network_fee: gaslessNetworkFee,
    protocol_fee: protocolFee,
    protocol_fee_rate: pair.fee_percentage,
    min_amount: pair.min_sats,
    max_amount: pair.max_sats,
    source_amount: btcSats.toString(),
    // `evmSmallest` is the delivered amount (the dex-quote folded the bridge
    // fee out); add it back so `target_amount` is the gross DEX output, matching
    // legacy's contract (`target_amount − bridge_fee = delivered`).
    target_amount: (evmSmallest + bridgeFee).toString(),
    net_source_amount: netSource.toString(),
    net_target_amount: netTarget.toString(),
    bridge_fee: bridgeFee > 0n ? Number(bridgeFee) : undefined,
  };
}

interface BtcEvmPivot {
  btcSats: bigint;
  evmSmallest: bigint;
  evmDecimals: number;
  /** Bridge fee deducted (target-token smallest units); `0n` if no bridge. */
  bridgeFee: bigint;
}

/**
 * BTC→EVM, source-pinned: user pins BTC sats; DEX runs exact-input
 * (tBTC pivot → target token) and returns the receivable target amount.
 */
async function dexSourcePinned(
  btcSats: bigint,
  params: ComposeQuoteParams,
  deps: ComposeQuoteDeps,
  btcPegged: { address: string; decimals: number },
  pivotChainId: number,
  tokenChainId: number,
  pivotScale: bigint,
): Promise<BtcEvmPivot> {
  const pivotInBase = btcSats * pivotScale;
  // `from` (pivot) and `to` (target token) share a chain when direct, and
  // straddle Arbitrum↔dest when bridged — the server bridges and folds the fee.
  const dexQuote = await deps.getDexQuote({
    from: { kind: "evm", chain_id: pivotChainId, address: btcPegged.address },
    to: {
      kind: "evm",
      chain_id: tokenChainId,
      address: params.targetToken.toLowerCase(),
    },
    amount: { kind: "exact_in", value: pivotInBase.toString() },
    slippageBps: params.slippageBps ?? 100,
  });
  return {
    btcSats,
    evmSmallest: BigInt(dexQuote.estimated_amount_out.raw),
    evmDecimals: dexQuote.estimated_amount_out.decimals,
    bridgeFee: BigInt(dexQuote.bridge_fee ?? "0"),
  };
}

/**
 * BTC→EVM, target-pinned: user pins target token amount; DEX runs
 * exact-output (tBTC pivot → target token) and returns the required
 * pivot amount. We divide that back into BTC sats for the fee math.
 *
 * Integer-floors the sats conversion to match server-side
 * `token_units_to_sats`. The DEX result is the precise tBTC needed; any
 * sub-sat dust is absorbed.
 */
async function dexTargetPinned(
  evmSmallest: bigint,
  params: ComposeQuoteParams,
  deps: ComposeQuoteDeps,
  btcPegged: { address: string; decimals: number },
  pivotChainId: number,
  tokenChainId: number,
  pivotScale: bigint,
): Promise<BtcEvmPivot> {
  // Bridged exact-out: the server grosses the DEX output up by the bridge fee
  // so `expected_amount_in` (the pivot needed) already covers it.
  const dexQuote = await deps.getDexQuote({
    from: { kind: "evm", chain_id: pivotChainId, address: btcPegged.address },
    to: {
      kind: "evm",
      chain_id: tokenChainId,
      address: params.targetToken.toLowerCase(),
    },
    amount: { kind: "exact_out", value: evmSmallest.toString() },
    slippageBps: params.slippageBps ?? 100,
  });
  const pivotInBase = BigInt(dexQuote.expected_amount_in.raw);
  return {
    btcSats: pivotInBase / pivotScale,
    evmSmallest,
    evmDecimals: dexQuote.estimated_amount_out.decimals,
    bridgeFee: BigInt(dexQuote.bridge_fee ?? "0"),
  };
}

function lookupPair(
  swapPairs: SwapPairsResponse,
  source: Chain,
  target: Chain,
) {
  const pair = swapPairs.pairs.find(
    (p) => p.source === source && p.target === target,
  );
  if (!pair) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: no swap-pair entry for ${source}→${target}`,
    );
  }
  return pair;
}

function lookupFeeEntry(
  networkFees: NetworkFeesResponse,
  source: Chain,
  target: Chain,
) {
  const entry = networkFees.pairs.find(
    (p) => p.source === source && p.target === target,
  );
  if (!entry) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: no network-fee entry for ${source}→${target}`,
    );
  }
  return entry;
}

async function composeEvmToBtc(
  params: ComposeQuoteParams,
  deps: ComposeQuoteDeps,
): Promise<QuoteResponse> {
  // Direct on a hub chain, or bridged in through Arbitrum for everything else.
  const { btcPegged, pivotChainId, hubChain, tokenChainId } = resolveEvmRouting(
    deps.chainConfig,
    params.sourceChain,
  );

  // The pivot↔BTC leg keys off the settlement hub — Arbitrum for a bridged
  // source, the source chain itself when direct.
  const pair = lookupPair(deps.swapPairs, hubChain, params.targetChain);
  const feeEntry = lookupFeeEntry(
    deps.networkFees,
    hubChain,
    params.targetChain,
  );
  const networkFeeSats = feeEntry.fees.source_sats + feeEntry.fees.target_sats;
  const gaslessNetworkFee = 0;

  // Pivot scale: BTC sats (8-dec) ↔ tBTC/WBTC base units. Same direction-
  // agnostic factor used by composeBtcToEvm.
  const pivotScale = 10n ** BigInt(btcPegged.decimals - 8);

  // Entry-point guarantees exactly one side is pinned; the trailing throw
  // is unreachable but keeps the amount typed as a non-null `bigint`.
  let pivot: EvmBtcPivot;
  if (isPivotToken(params.sourceToken, btcPegged)) {
    // Direct pegged token → BTC (e.g. tBTC → BTC): the source *is* the
    // pivot, no DEX hop — just the 1:1 BTC peg.
    pivot = directPeggedToBtcPivot(params, btcPegged, pivotScale);
  } else if (params.sourceAmount != null) {
    pivot = await dexEvmToBtcSourcePinned(
      BigInt(params.sourceAmount),
      params,
      deps,
      btcPegged,
      pivotChainId,
      tokenChainId,
      pivotScale,
    );
  } else if (params.targetAmount != null) {
    pivot = await dexEvmToBtcTargetPinned(
      BigInt(params.targetAmount),
      params,
      deps,
      btcPegged,
      pivotChainId,
      tokenChainId,
      pivotScale,
    );
  } else {
    throw new UnsupportedComposeQuotePath(
      "composeQuote: no amount pinned (unreachable)",
    );
  }

  const { btcSats, evmSmallest, evmDecimals, bridgeFee } = pivot;

  // Protocol fee in sats: floor(btc_sats × fee_percentage). Same fixed-
  // point trick as composeBtcToEvm.
  const FEE_SCALE = 10n ** 18n;
  const feePctScaled = BigInt(
    Math.round(pair.fee_percentage * Number(FEE_SCALE)),
  );
  const protocolFee = Number((btcSats * feePctScaled) / FEE_SCALE);
  const totalFeeSats = networkFeeSats + gaslessNetworkFee + protocolFee;

  // Net amounts mirror quote_calculator::compute_net_amounts for the
  // EVM→BTC arms.
  // - Source-pinned: source echoes pinned, net_target = btc_sats − fees.
  // - Target-pinned: target echoes pinned, net_source = sats_to_evm(btc_sats + fees).
  //   `sats_to_evm(s) = s × evm_smallest / btc_sats`, the implicit
  //   per-sat-to-evm ratio carried by the DEX quote.
  let netSource: bigint;
  let netTarget: bigint;
  if (params.sourceAmount != null) {
    netSource = evmSmallest;
    netTarget =
      btcSats > BigInt(totalFeeSats) ? btcSats - BigInt(totalFeeSats) : 0n;
  } else {
    const grossSats = btcSats + BigInt(totalFeeSats);
    netSource = btcSats === 0n ? 0n : (grossSats * evmSmallest) / btcSats;
    netTarget = btcSats;
  }

  return {
    exchange_rate: calculateExchangeRate(btcSats, evmSmallest, evmDecimals),
    network_fee: networkFeeSats,
    gasless_network_fee: gaslessNetworkFee,
    protocol_fee: protocolFee,
    protocol_fee_rate: pair.fee_percentage,
    min_amount: pair.min_sats,
    max_amount: pair.max_sats,
    // EVM→BTC: the EVM source is the bridged side. The dex-quote already
    // reports `evmSmallest` as the gross the user supplies (the inbound fee was
    // folded onto it), so `source_amount` needs no adjustment — just surface
    // the fee.
    source_amount: evmSmallest.toString(),
    target_amount: btcSats.toString(),
    net_source_amount: netSource.toString(),
    net_target_amount: netTarget.toString(),
    bridge_fee: bridgeFee > 0n ? Number(bridgeFee) : undefined,
  };
}

interface EvmBtcPivot {
  btcSats: bigint;
  evmSmallest: bigint;
  evmDecimals: number;
  /** Bridge fee deducted (source-token smallest units); `0n` if no bridge. */
  bridgeFee: bigint;
}

/**
 * EVM→BTC, source-pinned: user pins source token; DEX runs
 * exact-input (source → tBTC pivot) and returns the receivable pivot
 * amount, which divides back into BTC sats.
 */
async function dexEvmToBtcSourcePinned(
  evmSmallest: bigint,
  params: ComposeQuoteParams,
  deps: ComposeQuoteDeps,
  btcPegged: { address: string; decimals: number },
  pivotChainId: number,
  tokenChainId: number,
  pivotScale: bigint,
): Promise<EvmBtcPivot> {
  // `from` (source token) and `to` (pivot) share a chain when direct, and
  // straddle dest↔Arbitrum when bridged in — the server bridges the source USDC
  // to the hub and folds the inbound fee.
  const dexQuote = await deps.getDexQuote({
    from: {
      kind: "evm",
      chain_id: tokenChainId,
      address: params.sourceToken.toLowerCase(),
    },
    to: { kind: "evm", chain_id: pivotChainId, address: btcPegged.address },
    amount: { kind: "exact_in", value: evmSmallest.toString() },
    slippageBps: params.slippageBps ?? 100,
  });
  const pivotBase = BigInt(dexQuote.estimated_amount_out.raw);
  return {
    btcSats: pivotBase / pivotScale,
    evmSmallest,
    evmDecimals: dexQuote.expected_amount_in.decimals,
    bridgeFee: BigInt(dexQuote.bridge_fee ?? "0"),
  };
}

/**
 * EVM→BTC, target-pinned: user pins target BTC sats; DEX runs
 * exact-output (source → tBTC pivot) and returns the required source
 * amount. We inflate the pinned sats into pivot base units, ask the
 * DEX for the input cost, and use that as the pre-fee `evm_smallest`.
 *
 * Net source then re-scales `btc_sats + fees` through the same
 * implicit ratio (`evm_smallest / btc_sats`) the legacy server's
 * `sats_to_evm` helper uses, so source-side fee inflation matches.
 */
async function dexEvmToBtcTargetPinned(
  btcSats: bigint,
  params: ComposeQuoteParams,
  deps: ComposeQuoteDeps,
  btcPegged: { address: string; decimals: number },
  pivotChainId: number,
  tokenChainId: number,
  pivotScale: bigint,
): Promise<EvmBtcPivot> {
  const pivotInBase = btcSats * pivotScale;
  // Bridged-in exact-out: the server grosses the source USDC burn up by the
  // inbound fee, so `expected_amount_in` is what the user must supply on the
  // remote chain.
  const dexQuote = await deps.getDexQuote({
    from: {
      kind: "evm",
      chain_id: tokenChainId,
      address: params.sourceToken.toLowerCase(),
    },
    to: { kind: "evm", chain_id: pivotChainId, address: btcPegged.address },
    amount: { kind: "exact_out", value: pivotInBase.toString() },
    slippageBps: params.slippageBps ?? 100,
  });
  return {
    btcSats,
    evmSmallest: BigInt(dexQuote.expected_amount_in.raw),
    evmDecimals: dexQuote.expected_amount_in.decimals,
    bridgeFee: BigInt(dexQuote.bridge_fee ?? "0"),
  };
}

/**
 * Format the BTC→EVM exchange rate as a decimal string.
 *
 * Rate semantics: "how much of the EVM token you get per 1 BTC."
 * Concretely: `(evm_smallest / 10^evm_decimals) / (btc_sats / 10^8)`.
 *
 * Implementation does the division at high precision via `BigInt`
 * scaling, then formats with up to 12 fractional digits trimmed of
 * trailing zeros. The legacy server formats this with `rust_decimal`,
 * which preserves its own precision rules — a parity test should parse
 * both sides as numbers and compare with tolerance, not compare strings.
 */
function calculateExchangeRate(
  btcSats: bigint,
  evmSmallest: bigint,
  evmDecimals: number,
): string {
  if (btcSats === 0n) return "0";
  const PRECISION = 12;
  const scale = 10n ** BigInt(PRECISION);
  const numerator = evmSmallest * 10n ** 8n * scale;
  const denominator = btcSats * 10n ** BigInt(evmDecimals);
  const scaled = numerator / denominator;
  const intPart = scaled / scale;
  const fracPart = scaled % scale;
  let frac = fracPart.toString().padStart(PRECISION, "0");
  frac = frac.replace(/0+$/, "");
  return frac.length === 0 ? intPart.toString() : `${intPart}.${frac}`;
}
