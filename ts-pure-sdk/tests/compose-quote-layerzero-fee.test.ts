import { describe, expect, it } from "vitest";
import {
  type ComposeQuoteDeps,
  type ComposeQuoteParams,
  composeQuote,
} from "../src/compose-quote.js";
import type { Chain } from "../src/types/index.js";

const ARB_TBTC = "0x6c84a8f1c29108f47a79964b5fe888d4f4d0de40";
const PLASMA_USDT0 = "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb";
const LZ_FEE_SATS = 450;
const BASE_NETWORK_FEE_SATS = 50;

// 100k sats of tBTC (18 dec) buys 100 USDT0 (6 dec) on the DEX leg.
const DEX_OUT = 100_000_000n;
const SAT_IN_TBTC_BASE = 10_000_000_000n;

function deps(bridgeRate: unknown): ComposeQuoteDeps {
  const pairs = [{ source: "Bitcoin", target: "42161" }] as const;
  return {
    swapPairs: {
      pairs: pairs.map((p) => ({
        ...p,
        min_sats: 1000,
        max_sats: 100_000_000,
        fee_percentage: 0,
        network_fee_sats: 0,
      })),
    },
    networkFees: {
      pairs: pairs.map((p) => ({
        ...p,
        fees: { source_sats: BASE_NETWORK_FEE_SATS, target_sats: 0 },
      })),
    },
    chainConfig: {
      chains: [
        {
          chain: "42161",
          btc_pegged_token: { address: ARB_TBTC, decimals: 18, symbol: "tBTC" },
          btc_peg_rate: "1",
        },
      ],
    },
    getDexQuote: async ({ amount }) =>
      amount.kind === "exact_in"
        ? {
            expected_amount_in: { raw: amount.value, decimals: 18 },
            estimated_amount_out: { raw: DEX_OUT.toString(), decimals: 6 },
            bridge_rate: bridgeRate as never,
          }
        : {
            expected_amount_in: {
              raw: (100_000n * SAT_IN_TBTC_BASE).toString(),
              decimals: 18,
            },
            estimated_amount_out: { raw: amount.value, decimals: 6 },
            bridge_rate: bridgeRate as never,
          },
  };
}

const sourcePinned: ComposeQuoteParams = {
  sourceChain: "Bitcoin",
  sourceToken: "btc",
  targetChain: "9745" as Chain,
  targetToken: PLASMA_USDT0,
  sourceAmount: 100_000,
};

describe("composeQuote — LayerZero bridge fee", () => {
  it("adds the LayerZero fee to the network fee and nets it out of the target", async () => {
    const q = await composeQuote(
      sourcePinned,
      deps({ router: "layerzero", native_fee_sats: String(LZ_FEE_SATS) }),
    );

    expect(q.network_fee).toBe(BASE_NETWORK_FEE_SATS + LZ_FEE_SATS);
    // No token deduction: the USDT0 is bridged in full, the fee is in sats.
    expect(q.bridge_fee).toBeUndefined();
    const effectiveSats =
      100_000n - BigInt(BASE_NETWORK_FEE_SATS + LZ_FEE_SATS);
    expect(q.net_target_amount).toBe(
      ((effectiveSats * DEX_OUT) / 100_000n).toString(),
    );
  });

  it("charges the LayerZero fee on top of the source for a target-pinned quote", async () => {
    const q = await composeQuote(
      { ...sourcePinned, sourceAmount: undefined, targetAmount: 100_000_000 },
      deps({ router: "layerzero", native_fee_sats: String(LZ_FEE_SATS) }),
    );

    expect(q.network_fee).toBe(BASE_NETWORK_FEE_SATS + LZ_FEE_SATS);
    expect(q.net_source_amount).toBe(
      String(100_000 + BASE_NETWORK_FEE_SATS + LZ_FEE_SATS),
    );
  });

  it("leaves the network fee alone without a LayerZero leg", async () => {
    const q = await composeQuote(
      { ...sourcePinned, targetChain: "42161" },
      deps(undefined),
    );
    expect(q.network_fee).toBe(BASE_NETWORK_FEE_SATS);
  });
});
