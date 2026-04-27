/**
 * Map EVM chain IDs to CCTPv2 source-chain metadata (domain + USDC
 * address) for the consuming side of the CCTP-inbound flow.
 *
 * Callers pass `signer.chainId`; the SDK derives the CCTP domain +
 * the source-chain USDC address from that alone, so the public API
 * doesn't need a redundant "sourceChain" string.
 */

import type { Chain, Hex } from "viem";
import {
  arbitrum,
  avalanche,
  base,
  hyperEvm,
  ink,
  linea,
  mainnet,
  monad,
  optimism,
  polygon,
  sei,
  sonic,
  unichain,
  worldchain,
} from "viem/chains";
import {
  CCTP_DOMAINS,
  type CctpChainName,
  USDC_ADDRESSES,
} from "../cctp/constants.js";

/** Canonical EVM chain id → CCTP chain name. */
export const CHAIN_ID_TO_CCTP_NAME: Record<number, CctpChainName> = {
  1: "Ethereum",
  10: "Optimism",
  130: "Unichain",
  137: "Polygon",
  146: "Sonic",
  1329: "Sei",
  8453: "Base",
  42161: "Arbitrum",
  43114: "Avalanche",
  59144: "Linea",
  480: "World Chain",
  57073: "Ink",
  999: "HyperEVM",
  10143: "Monad",
  // Solana lands later (non-EVM, needs a different signer path).
};

/**
 * EVM chain ids the Lendaswap backend accepts directly as a swap
 * source via Permit2 — these chains are also CCTP-supported, but
 * we never route them through CCTP since a direct-Permit2 funding
 * is cheaper and faster.
 */
export const DIRECT_SOURCE_CHAIN_IDS: ReadonlySet<number> = new Set([
  1, // Ethereum
  137, // Polygon
  42161, // Arbitrum
]);

/**
 * `true` when `chainId` is a CCTP-supported source that the backend
 * does NOT accept directly — i.e. funding a swap from this chain
 * requires a CCTP hop to Arbitrum before the HTLC is created.
 */
export function isCctpOnlySource(chainId: number): boolean {
  return (
    chainId in CHAIN_ID_TO_CCTP_NAME && !DIRECT_SOURCE_CHAIN_IDS.has(chainId)
  );
}

/**
 * Look up CCTPv2 domain id + native USDC address for a given chain id.
 * Throws with a clear error if the chain is unsupported.
 */
export function cctpMetaForChainId(chainId: number): {
  name: CctpChainName;
  domain: number;
  usdc: Hex;
} {
  const name = CHAIN_ID_TO_CCTP_NAME[chainId];
  if (!name) {
    throw new Error(
      `Chain id ${chainId} is not a supported CCTP source chain. Supported ids: ${Object.keys(CHAIN_ID_TO_CCTP_NAME).join(", ")}.`,
    );
  }
  const usdc = USDC_ADDRESSES[name];
  if (!usdc) {
    throw new Error(
      `CCTP chain ${name} has no native USDC address in the SDK registry.`,
    );
  }
  return {
    name,
    domain: CCTP_DOMAINS[name],
    usdc: usdc as Hex,
  };
}

/**
 * Map of CCTP source chain id → viem `Chain` object. Mirrors
 * [`CHAIN_ID_TO_CCTP_NAME`] — keep them in lockstep when adding chains.
 *
 * Exposed so browser consumers (wagmi, viem clients) don't have to maintain
 * their own parallel mapping; tree-shakable so non-browser consumers don't
 * pay the bundle cost.
 */
export const CCTP_VIEM_CHAINS: Record<number, Chain> = {
  1: mainnet,
  10: optimism,
  130: unichain,
  137: polygon,
  146: sonic,
  1329: sei,
  8453: base,
  42161: arbitrum,
  43114: avalanche,
  59144: linea,
  480: worldchain,
  57073: ink,
  999: hyperEvm,
  10143: monad,
};

/** Look up the viem `Chain` for a CCTP source chain id, or `undefined`. */
export function getCctpViemChain(chainId: number): Chain | undefined {
  return CCTP_VIEM_CHAINS[chainId];
}

/** Look up the viem `Chain` for a CCTP source chain name, or `undefined`. */
export function getCctpViemChainByName(name: string): Chain | undefined {
  for (const [chainId, chainName] of Object.entries(CHAIN_ID_TO_CCTP_NAME)) {
    if (chainName === name) return CCTP_VIEM_CHAINS[Number(chainId)];
  }
  return undefined;
}
