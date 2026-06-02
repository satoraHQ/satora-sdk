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
 * # Current scope (v0)
 *
 *  - BTC (`Bitcoin` chain, source_token=`"btc"`) ↔ EVM token on **Arbitrum**.
 *  - Source-amount pinned (`sourceAmount` set).
 *
 * Everything else throws `UnsupportedComposeQuotePath` — callers should
 * fall back to `getQuote()` for those for now.
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
function pivotForChain(
  chainConfig: ChainConfigResponse,
  chain: Chain,
): { address: string; decimals: number } | undefined {
  const entry = chainConfig.chains.find((c) => c.chain === chain);
  if (!entry) return undefined;
  return {
    address: entry.btc_pegged_token.address,
    decimals: entry.btc_pegged_token.decimals,
  };
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
  /** Slippage tolerance for the DEX leg (bps). Defaults to 50 (= 0.5%). */
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
  const evmChain = params.targetChain;
  const btcPegged = pivotForChain(deps.chainConfig, evmChain);
  if (!btcPegged) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: no BTC-pegged token configured for chain ${evmChain}`,
    );
  }
  const chainIdNum = Number.parseInt(evmChain, 10);
  if (Number.isNaN(chainIdNum)) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: target chain ${evmChain} is not numeric (EVM)`,
    );
  }

  const pair = lookupPair(
    deps.swapPairs,
    params.sourceChain,
    params.targetChain,
  );
  const feeEntry = lookupFeeEntry(
    deps.networkFees,
    params.sourceChain,
    params.targetChain,
  );
  const networkFeeSats = feeEntry.fees.source_sats + feeEntry.fees.target_sats;
  // L2s (Polygon, Arbitrum) don't charge a gasless overhead; only mainnet
  // Ethereum does. composeQuote v0 doesn't ship Ethereum yet so it's
  // always 0 here, but the field is wired through to the caller's
  // QuoteResponse for when it does.
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
  if (params.sourceAmount != null) {
    pivot = await dexSourcePinned(
      BigInt(params.sourceAmount),
      params,
      deps,
      btcPegged,
      chainIdNum,
      pivotScale,
    );
  } else if (params.targetAmount != null) {
    pivot = await dexTargetPinned(
      BigInt(params.targetAmount),
      params,
      deps,
      btcPegged,
      chainIdNum,
      pivotScale,
    );
  } else {
    throw new UnsupportedComposeQuotePath(
      "composeQuote: no amount pinned (unreachable)",
    );
  }

  const { btcSats, evmSmallest, evmDecimals } = pivot;

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
    target_amount: evmSmallest.toString(),
    net_source_amount: netSource.toString(),
    net_target_amount: netTarget.toString(),
    bridge_fee: undefined,
  };
}

interface BtcEvmPivot {
  btcSats: bigint;
  evmSmallest: bigint;
  evmDecimals: number;
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
  chainIdNum: number,
  pivotScale: bigint,
): Promise<BtcEvmPivot> {
  const pivotInBase = btcSats * pivotScale;
  const dexQuote = await deps.getDexQuote({
    from: { kind: "evm", chain_id: chainIdNum, address: btcPegged.address },
    to: {
      kind: "evm",
      chain_id: chainIdNum,
      address: params.targetToken.toLowerCase(),
    },
    amount: { kind: "exact_in", value: pivotInBase.toString() },
    slippageBps: params.slippageBps ?? 100,
  });
  return {
    btcSats,
    evmSmallest: BigInt(dexQuote.estimated_amount_out.raw),
    evmDecimals: dexQuote.estimated_amount_out.decimals,
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
  chainIdNum: number,
  pivotScale: bigint,
): Promise<BtcEvmPivot> {
  const dexQuote = await deps.getDexQuote({
    from: { kind: "evm", chain_id: chainIdNum, address: btcPegged.address },
    to: {
      kind: "evm",
      chain_id: chainIdNum,
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
  const evmChain = params.sourceChain;
  const btcPegged = pivotForChain(deps.chainConfig, evmChain);
  if (!btcPegged) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: no BTC-pegged token configured for chain ${evmChain}`,
    );
  }
  const chainIdNum = Number.parseInt(evmChain, 10);
  if (Number.isNaN(chainIdNum)) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: source chain ${evmChain} is not numeric (EVM)`,
    );
  }

  const pair = lookupPair(
    deps.swapPairs,
    params.sourceChain,
    params.targetChain,
  );
  const feeEntry = lookupFeeEntry(
    deps.networkFees,
    params.sourceChain,
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
  if (params.sourceAmount != null) {
    pivot = await dexEvmToBtcSourcePinned(
      BigInt(params.sourceAmount),
      params,
      deps,
      btcPegged,
      chainIdNum,
      pivotScale,
    );
  } else if (params.targetAmount != null) {
    pivot = await dexEvmToBtcTargetPinned(
      BigInt(params.targetAmount),
      params,
      deps,
      btcPegged,
      chainIdNum,
      pivotScale,
    );
  } else {
    throw new UnsupportedComposeQuotePath(
      "composeQuote: no amount pinned (unreachable)",
    );
  }

  const { btcSats, evmSmallest, evmDecimals } = pivot;

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
    source_amount: evmSmallest.toString(),
    target_amount: btcSats.toString(),
    net_source_amount: netSource.toString(),
    net_target_amount: netTarget.toString(),
    bridge_fee: undefined,
  };
}

interface EvmBtcPivot {
  btcSats: bigint;
  evmSmallest: bigint;
  evmDecimals: number;
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
  chainIdNum: number,
  pivotScale: bigint,
): Promise<EvmBtcPivot> {
  const dexQuote = await deps.getDexQuote({
    from: {
      kind: "evm",
      chain_id: chainIdNum,
      address: params.sourceToken.toLowerCase(),
    },
    to: { kind: "evm", chain_id: chainIdNum, address: btcPegged.address },
    amount: { kind: "exact_in", value: evmSmallest.toString() },
    slippageBps: params.slippageBps ?? 100,
  });
  const pivotBase = BigInt(dexQuote.estimated_amount_out.raw);
  return {
    btcSats: pivotBase / pivotScale,
    evmSmallest,
    evmDecimals: dexQuote.expected_amount_in.decimals,
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
  chainIdNum: number,
  pivotScale: bigint,
): Promise<EvmBtcPivot> {
  const pivotInBase = btcSats * pivotScale;
  const dexQuote = await deps.getDexQuote({
    from: {
      kind: "evm",
      chain_id: chainIdNum,
      address: params.sourceToken.toLowerCase(),
    },
    to: { kind: "evm", chain_id: chainIdNum, address: btcPegged.address },
    amount: { kind: "exact_out", value: pivotInBase.toString() },
    slippageBps: params.slippageBps ?? 100,
  });
  return {
    btcSats,
    evmSmallest: BigInt(dexQuote.expected_amount_in.raw),
    evmDecimals: dexQuote.expected_amount_in.decimals,
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
