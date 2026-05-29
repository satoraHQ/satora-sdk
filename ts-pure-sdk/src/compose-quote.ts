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
  if (params.sourceAmount == null) {
    throw new UnsupportedComposeQuotePath(
      "composeQuote v0 only supports source-amount-pinned quotes; target-amount-pinned coming later",
    );
  }
  if (params.targetAmount != null) {
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

  if (params.sourceAmount === undefined) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: target source amount is undefined`,
    );
  }

  const btcSats = BigInt(params.sourceAmount);

  // --- primitive 1: /swap-pairs (static) ---
  const pair = deps.swapPairs.pairs.find(
    (p) => p.source === params.sourceChain && p.target === params.targetChain,
  );
  if (!pair) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: no swap-pair entry for ${params.sourceChain}→${params.targetChain}`,
    );
  }

  // --- primitive 2: /network-fees (dynamic, ~15s freshness) ---
  const feeEntry = deps.networkFees.pairs.find(
    (p) => p.source === params.sourceChain && p.target === params.targetChain,
  );
  if (!feeEntry) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: no network-fee entry for ${params.sourceChain}→${params.targetChain}`,
    );
  }
  const networkFeeSats = feeEntry.fees.source_sats + feeEntry.fees.target_sats;
  // L2s (Polygon, Arbitrum) don't charge a gasless overhead; only mainnet
  // Ethereum does. v0 supports Arbitrum only, so always 0 here.
  const gaslessNetworkFee = 0;

  // --- primitive 3: /dex-quote (live, debounced in UIs) ---
  // BTC sats → tBTC base units. tBTC is 18-dec, sat is 8-dec → ×10^10.
  const pivotInBase = btcSats * 10n ** BigInt(btcPegged.decimals - 8);
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

  const evmSmallest = BigInt(dexQuote.estimated_amount_out.raw);
  const evmDecimals = dexQuote.estimated_amount_out.decimals;

  // --- protocol fee + net-amount composition ---
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

  // BTC→EVM, source-pinned: source = btc_sats (unchanged), target =
  // sats_to_evm(btc_sats − fees). Same shape as
  // quote_calculator::compute_net_amounts for the (true, Source) arm.
  const effectiveSats = btcSats - BigInt(totalFeeSats);
  const netTarget =
    btcSats === 0n
      ? 0n
      : (effectiveSats < 0n ? 0n : effectiveSats * evmSmallest) / btcSats;

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
    net_source_amount: btcSats.toString(),
    net_target_amount: netTarget.toString(),
    bridge_fee: undefined,
  };
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
  if (params.sourceAmount === undefined) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: source amount is undefined`,
    );
  }

  const evmSmallest = BigInt(params.sourceAmount);

  // --- primitive 1: /swap-pairs (static) ---
  const pair = deps.swapPairs.pairs.find(
    (p) => p.source === params.sourceChain && p.target === params.targetChain,
  );
  if (!pair) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: no swap-pair entry for ${params.sourceChain}→${params.targetChain}`,
    );
  }

  // --- primitive 2: /network-fees (dynamic, ~15s freshness) ---
  const feeEntry = deps.networkFees.pairs.find(
    (p) => p.source === params.sourceChain && p.target === params.targetChain,
  );
  if (!feeEntry) {
    throw new UnsupportedComposeQuotePath(
      `composeQuote: no network-fee entry for ${params.sourceChain}→${params.targetChain}`,
    );
  }
  const networkFeeSats = feeEntry.fees.source_sats + feeEntry.fees.target_sats;
  const gaslessNetworkFee = 0;

  // --- primitive 3: /dex-quote (source EVM token → tBTC) ---
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

  // tBTC base units → BTC sats. tBTC is 18-dec, sat is 8-dec → ÷10^10.
  // The DEX result is the precise tBTC amount; integer-divide trims sub-sat
  // dust (matching the server's `token_units_to_sats` for the BTC-pegged
  // token-list entry).
  const tbtcBase = BigInt(dexQuote.estimated_amount_out.raw);
  const scale = 10n ** BigInt(btcPegged.decimals - 8);
  const btcSats = tbtcBase / scale;

  // --- protocol fee + net-amount composition ---
  const FEE_SCALE = 10n ** 18n;
  const feePctScaled = BigInt(
    Math.round(pair.fee_percentage * Number(FEE_SCALE)),
  );
  const protocolFee = Number((btcSats * feePctScaled) / FEE_SCALE);

  const totalFeeSats = networkFeeSats + gaslessNetworkFee + protocolFee;

  // EVM→BTC, source-pinned: source = evm_smallest (echoed), target =
  // btc_sats (pre-fee DEX output), net_target = btc_sats − fees.
  // Mirrors quote_calculator::compute_net_amounts for the (false, Source)
  // arm.
  const netTargetSats =
    btcSats > BigInt(totalFeeSats) ? btcSats - BigInt(totalFeeSats) : 0n;

  return {
    exchange_rate: calculateExchangeRate(
      btcSats,
      evmSmallest,
      dexQuote.expected_amount_in.decimals,
    ),
    network_fee: networkFeeSats,
    gasless_network_fee: gaslessNetworkFee,
    protocol_fee: protocolFee,
    protocol_fee_rate: pair.fee_percentage,
    min_amount: pair.min_sats,
    max_amount: pair.max_sats,
    source_amount: evmSmallest.toString(),
    target_amount: btcSats.toString(),
    net_source_amount: evmSmallest.toString(),
    net_target_amount: netTargetSats.toString(),
    bridge_fee: undefined,
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
