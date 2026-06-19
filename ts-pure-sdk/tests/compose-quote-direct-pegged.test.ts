import { describe, expect, it } from "vitest";
import {
  type ComposeQuoteDeps,
  type ComposeQuoteParams,
  composeQuote,
} from "../src/compose-quote.js";

// tBTC on Arbitrum — 18-dec pegged pivot, redeemable 1:1, so the direct
// BTC↔tBTC conversion is pure decimal scaling (pivotScale = 10^(18-8)).
const ARB_TBTC = "0x6c84a8f1c29108f47a79964b5fe888d4f4d0de40";
// WBTC on Polygon — 8-dec pegged pivot priced at a non-1:1 rate, so the
// conversion exercises `parseRate` + the rounding (pivotScale = 10^0 = 1).
const POL_WBTC = "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6";

// 1 BTC sat scales to 10^10 tBTC base units (18 − 8 decimals).
const SAT_IN_TBTC_BASE = 10_000_000_000;

/**
 * Build `ComposeQuoteDeps` with zero fees by default so the conversion math
 * is isolated; individual tests override `swapPairs`/`networkFees` to also
 * cover the fee path. `getDexQuote` throws — the direct-pegged path must
 * never reach the DEX.
 */
function deps(overrides: Partial<ComposeQuoteDeps> = {}): ComposeQuoteDeps {
  const zeroPairs = [
    { source: "Bitcoin", target: "42161" },
    { source: "42161", target: "Bitcoin" },
    { source: "Bitcoin", target: "137" },
    { source: "137", target: "Bitcoin" },
  ] as const;

  return {
    swapPairs: {
      pairs: zeroPairs.map((p) => ({
        source: p.source,
        target: p.target,
        min_sats: 1000,
        max_sats: 100_000_000,
        fee_percentage: 0,
      })),
    },
    networkFees: {
      pairs: zeroPairs.map((p) => ({
        source: p.source,
        target: p.target,
        fees: { source_sats: 0, target_sats: 0 },
      })),
    },
    chainConfig: {
      chains: [
        {
          chain: "42161",
          btc_pegged_token: { address: ARB_TBTC, decimals: 18, symbol: "tBTC" },
          btc_peg_rate: "1",
        },
        {
          chain: "137",
          btc_pegged_token: { address: POL_WBTC, decimals: 8, symbol: "WBTC" },
          btc_peg_rate: "0.99790000",
        },
      ],
    },
    getDexQuote: async () => {
      throw new Error(
        "getDexQuote must not be called on the direct-pegged path",
      );
    },
    ...overrides,
  };
}

describe("composeQuote — direct BTC↔pegged pivot (no DEX)", () => {
  it("BTC → tBTC, source-pinned: scales sats to 18-dec base units (1:1)", async () => {
    const params: ComposeQuoteParams = {
      sourceChain: "Bitcoin",
      sourceToken: "btc",
      targetChain: "42161",
      targetToken: ARB_TBTC,
      sourceAmount: 100_000,
    };
    const q = await composeQuote(params, deps());

    expect(q.source_amount).toBe("100000");
    expect(q.target_amount).toBe((100_000 * SAT_IN_TBTC_BASE).toString());
    expect(q.exchange_rate).toBe("1");
    expect(q.network_fee).toBe(0);
    expect(q.gasless_network_fee).toBe(0);
    expect(q.protocol_fee).toBe(0);
    // No fees → net target equals the gross target.
    expect(q.net_target_amount).toBe(q.target_amount);
    expect(q.net_source_amount).toBe("100000");
  });

  it("tBTC → BTC, source-pinned: divides base units back to sats (1:1)", async () => {
    const params: ComposeQuoteParams = {
      sourceChain: "42161",
      sourceToken: ARB_TBTC,
      targetChain: "Bitcoin",
      targetToken: "btc",
      sourceAmount: 100_000 * SAT_IN_TBTC_BASE,
    };
    const q = await composeQuote(params, deps());

    expect(q.source_amount).toBe((100_000 * SAT_IN_TBTC_BASE).toString());
    expect(q.target_amount).toBe("100000");
    expect(q.exchange_rate).toBe("1");
    expect(q.net_target_amount).toBe("100000");
  });

  it("BTC → tBTC, target-pinned: solves required sats from the pinned token amount", async () => {
    const params: ComposeQuoteParams = {
      sourceChain: "Bitcoin",
      sourceToken: "btc",
      targetChain: "42161",
      targetToken: ARB_TBTC,
      targetAmount: 100_000 * SAT_IN_TBTC_BASE,
    };
    const q = await composeQuote(params, deps());

    // Pinned target echoes through; source is the sats needed to back it.
    expect(q.target_amount).toBe((100_000 * SAT_IN_TBTC_BASE).toString());
    expect(q.source_amount).toBe("100000");
    expect(q.net_target_amount).toBe(q.target_amount);
    expect(q.net_source_amount).toBe("100000");
  });

  it("round-trips losslessly across the 1:1 peg", async () => {
    const forward = await composeQuote(
      {
        sourceChain: "Bitcoin",
        sourceToken: "btc",
        targetChain: "42161",
        targetToken: ARB_TBTC,
        sourceAmount: 100_000,
      },
      deps(),
    );
    const back = await composeQuote(
      {
        sourceChain: "42161",
        sourceToken: ARB_TBTC,
        targetChain: "Bitcoin",
        targetToken: "btc",
        sourceAmount: Number(forward.target_amount),
      },
      deps(),
    );
    expect(back.target_amount).toBe("100000");
  });

  it("BTC → WBTC: applies the non-1:1 peg rate with round-half-up", async () => {
    const params: ComposeQuoteParams = {
      sourceChain: "Bitcoin",
      sourceToken: "btc",
      targetChain: "137",
      targetToken: POL_WBTC,
      sourceAmount: 100_000,
    };
    const q = await composeQuote(params, deps());

    // 100_000 sats × 0.9979 = 99_790 WBTC base units (8-dec, pivotScale 1).
    expect(q.target_amount).toBe("99790");
    expect(q.exchange_rate).toBe("0.9979");
  });

  it("truncates sub-sat dust on the inverse conversion", async () => {
    // Half a sat's worth of tBTC base units → truncates to 0 sats.
    const params: ComposeQuoteParams = {
      sourceChain: "42161",
      sourceToken: ARB_TBTC,
      targetChain: "Bitcoin",
      targetToken: "btc",
      sourceAmount: SAT_IN_TBTC_BASE / 2,
    };
    const q = await composeQuote(params, deps());

    expect(q.target_amount).toBe("0");
    expect(q.exchange_rate).toBe("0");
  });

  it("applies network + protocol fees on the BTC side (source-pinned)", async () => {
    const overrides: Partial<ComposeQuoteDeps> = {
      swapPairs: {
        pairs: [
          {
            source: "Bitcoin",
            target: "42161",
            min_sats: 1000,
            max_sats: 100_000_000,
            fee_percentage: 0.0025,
          },
        ],
      },
      networkFees: {
        pairs: [
          {
            source: "Bitcoin",
            target: "42161",
            fees: { source_sats: 500, target_sats: 300 },
          },
        ],
      },
    };
    const q = await composeQuote(
      {
        sourceChain: "Bitcoin",
        sourceToken: "btc",
        targetChain: "42161",
        targetToken: ARB_TBTC,
        sourceAmount: 100_000,
      },
      deps(overrides),
    );

    expect(q.network_fee).toBe(800); // 500 + 300
    expect(q.protocol_fee).toBe(250); // floor(100_000 × 0.0025)
    expect(q.protocol_fee_rate).toBe(0.0025);
    // net_target = (sats − totalFees) × evmSmallest / sats
    //            = (100_000 − 1_050) × 10^10
    expect(q.net_target_amount).toBe((98_950 * SAT_IN_TBTC_BASE).toString());
  });
});
