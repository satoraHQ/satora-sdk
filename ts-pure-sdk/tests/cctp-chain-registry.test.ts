/**
 * CCTP chain registry consistency.
 *
 * Per-chain CCTP metadata is split across four maps that all need to stay in
 * lockstep:
 *
 *   - `CCTP_DOMAINS`           — name → CCTP domain id
 *   - `USDC_ADDRESSES`         — name → native USDC address
 *   - `CHAIN_ID_TO_CCTP_NAME`  — EVM chain id → name (CCTP-supported EVM chains)
 *   - `CCTP_VIEM_CHAINS`       — EVM chain id → viem `Chain`
 *
 * A previous bug shipped Monad in the first three but missed
 * `CHAIN_ID_TO_CCTP_NAME`, leaving Monad selectable in the UI but unroutable
 * through CCTP create/fund. These tests would have caught it.
 *
 * Solana is the only intentional non-EVM entry — it lives in `CCTP_DOMAINS` /
 * `USDC_ADDRESSES` but not in the chain-id maps.
 */

import { describe, expect, it } from "vitest";
import {
  CCTP_DOMAINS,
  CCTP_VIEM_CHAINS,
  CHAIN_ID_TO_CCTP_NAME,
  USDC_ADDRESSES,
} from "../src/index.js";

const NON_EVM_CHAINS = new Set(["Solana"]);

describe("CCTP chain registry consistency", () => {
  it("every EVM CCTP_DOMAINS entry has a USDC address", () => {
    for (const name of Object.keys(CCTP_DOMAINS)) {
      if (NON_EVM_CHAINS.has(name)) continue;
      expect(USDC_ADDRESSES[name], `USDC address missing for ${name}`).toMatch(
        /^0x[0-9a-fA-F]{40}$/,
      );
    }
  });

  it("every CHAIN_ID_TO_CCTP_NAME entry has matching CCTP_DOMAINS / USDC_ADDRESSES / CCTP_VIEM_CHAINS", () => {
    for (const [chainIdStr, name] of Object.entries(CHAIN_ID_TO_CCTP_NAME)) {
      const chainId = Number(chainIdStr);

      expect(
        CCTP_DOMAINS[name as keyof typeof CCTP_DOMAINS],
        `CCTP_DOMAINS missing for ${name}`,
      ).toBeDefined();

      expect(
        USDC_ADDRESSES[name],
        `USDC_ADDRESSES missing for ${name}`,
      ).toBeDefined();

      const viemChain = CCTP_VIEM_CHAINS[chainId];
      expect(
        viemChain,
        `CCTP_VIEM_CHAINS missing for chain id ${chainId} (${name})`,
      ).toBeDefined();
      expect(
        viemChain.id,
        `viem chain id mismatch for ${name}: registry says ${chainId}, viem says ${viemChain.id}`,
      ).toBe(chainId);
    }
  });

  it("every EVM chain in CCTP_DOMAINS has a chain-id mapping", () => {
    const mappedNames = new Set(Object.values(CHAIN_ID_TO_CCTP_NAME));
    for (const name of Object.keys(CCTP_DOMAINS)) {
      if (NON_EVM_CHAINS.has(name)) continue;
      expect(
        mappedNames.has(name as (typeof CHAIN_ID_TO_CCTP_NAME)[number]),
        `CHAIN_ID_TO_CCTP_NAME missing entry for ${name} — UI may show it but CCTP routing will fail`,
      ).toBe(true);
    }
  });

  it("CCTP_VIEM_CHAINS keys match CHAIN_ID_TO_CCTP_NAME keys", () => {
    const viemKeys = Object.keys(CCTP_VIEM_CHAINS).sort();
    const nameKeys = Object.keys(CHAIN_ID_TO_CCTP_NAME).sort();
    expect(viemKeys).toEqual(nameKeys);
  });
});
