import type { Chain, TokenId, TokenInfo } from "./api/client.js";
import { CCTP_DOMAINS, USDC_ADDRESSES } from "./cctp/index.js";
import { LZ_EIDS, USDT0_ADDRESSES } from "./usdt0-bridge/index.js";

/** A token identifier: either a plain string TokenId or a TokenInfo object. */
export type TokenInput = TokenId;

// ── Asset ────────────────────────────────────────────────────────────────────

/**
 * Minimal asset identifier — just a chain and token ID.
 *
 * Use the predefined constants in {@link Asset} for common tokens,
 * or construct your own for any token the API supports:
 *
 * ```ts
 * // Predefined
 * Asset.BTC_ARKADE
 * Asset.USDC_POLYGON
 *
 * // Custom (any ERC-20 by contract address)
 * { chain: "137", tokenId: "0x..." }
 * ```
 */
export interface Asset {
  /** Chain identifier — e.g. "Lightning", "Arkade", "Bitcoin", "137", "1", "42161" */
  chain: Chain | (string & {});
  /** Token ID — "btc" for Bitcoin, or the ERC-20 contract address for EVM tokens */
  tokenId: string;
}

// Well-known USDC contract addresses
const USDC_POLYGON = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";
const USDC_ARBITRUM = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";
const USDC_ETHEREUM = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";

// Well-known USDT contract addresses
const USDT_POLYGON = "0xc2132D05D31c914a87C6611C10748AEb04B58e8F";
const USDT_ARBITRUM = "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9";
const USDT_ETHEREUM = "0xdAC17F958D2ee523a2206206994597C13D831ec7";

// Well-known WBTC contract addresses
const WBTC_POLYGON = "0x1BFD67037B42Cf73acF2047067bd4F2C47D9BfD6";
const WBTC_ETHEREUM = "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599";

// Well-known tBTC contract addresses
const TBTC_ETHEREUM = "0x18084fbA666a33d37592fA2633fD49a74DD93a88";
const TBTC_ARBITRUM = "0x6c84a8f1c29108F47a79964b5Fe888D4f4D0dE40";

/**
 * Predefined asset constants for common tokens.
 *
 * ```ts
 * import { Asset } from "@lendasat/lendaswap-sdk-pure";
 *
 * await client.createSwap({
 *   source: Asset.BTC_ARKADE,
 *   target: Asset.USDC_POLYGON,
 *   sourceAmount: 100_000,
 *   targetAddress: "0x...",
 * });
 * ```
 */
export const Asset = {
  // Bitcoin
  BTC_LIGHTNING: { chain: "Lightning", tokenId: "btc" } as Asset,
  BTC_ARKADE: { chain: "Arkade", tokenId: "btc" } as Asset,
  BTC_ONCHAIN: { chain: "Bitcoin", tokenId: "btc" } as Asset,

  // USDC
  USDC_POLYGON: { chain: "137", tokenId: USDC_POLYGON } as Asset,
  USDC_ARBITRUM: { chain: "42161", tokenId: USDC_ARBITRUM } as Asset,
  USDC_ETHEREUM: { chain: "1", tokenId: USDC_ETHEREUM } as Asset,

  // USDT
  USDT_POLYGON: { chain: "137", tokenId: USDT_POLYGON } as Asset,
  USDT_ARBITRUM: { chain: "42161", tokenId: USDT_ARBITRUM } as Asset,
  USDT_ETHEREUM: { chain: "1", tokenId: USDT_ETHEREUM } as Asset,

  // WBTC
  WBTC_POLYGON: { chain: "137", tokenId: WBTC_POLYGON } as Asset,
  WBTC_ETHEREUM: { chain: "1", tokenId: WBTC_ETHEREUM } as Asset,

  // tBTC
  TBTC_ETHEREUM: { chain: "1", tokenId: TBTC_ETHEREUM } as Asset,
  TBTC_ARBITRUM: { chain: "42161", tokenId: TBTC_ARBITRUM } as Asset,
} as const;

// ── Legacy constants (kept for backward compatibility) ───────────────────────

// Well-known token ID constants
export const BTC_LIGHTNING: TokenId = "btc";
export const BTC_ARKADE: TokenId = "btc";
export const BTC_ONCHAIN: TokenId = "btc";

// Well-known TokenInfo constants
export const BTC_LIGHTNING_INFO: TokenInfo = {
  token_id: BTC_LIGHTNING,
  symbol: "BTC",
  name: "Bitcoin (Lightning)",
  decimals: 8,
  chain: "Lightning",
};

export const BTC_ARKADE_INFO: TokenInfo = {
  token_id: BTC_ARKADE,
  symbol: "BTC",
  name: "Bitcoin (Arkade)",
  decimals: 8,
  chain: "Arkade",
};

export const BTC_ONCHAIN_INFO: TokenInfo = {
  token_id: BTC_ONCHAIN,
  symbol: "BTC",
  name: "Bitcoin (On-chain)",
  decimals: 8,
  chain: "Bitcoin",
};

// ============================================================================
// EVM Chain IDs
// ============================================================================

/** Source chains where HTLCs and DEX swaps run. */
const SOURCE_EVM_CHAINS = ["1", "137", "42161"] as const;

/** All EVM chain IDs including CCTP and USDT0 bridge-only destinations. */
const ALL_EVM_CHAIN_IDS: Record<string, string> = {
  Ethereum: "1",
  Polygon: "137",
  Arbitrum: "42161",
  Optimism: "10",
  Base: "8453",
  Avalanche: "43114",
  Linea: "59144",
  Unichain: "130",
  Sonic: "146",
  "World Chain": "480",
  Ink: "57073",
  Sei: "1329",
  HyperEVM: "999",
  Monad: "143",
  // USDT0-only chains
  Berachain: "80094",
  "Conflux eSpace": "1030",
  Corn: "21000000",
  Flare: "14",
  Hedera: "295",
  Mantle: "5000",
  MegaETH: "4326",
  Morph: "2818",
  Plasma: "9745",
  Rootstock: "30",
  Stable: "988",
  Tempo: "4217",
  XLayer: "196",
};

/** Reverse lookup: chain ID → chain name. */
const CHAIN_ID_TO_NAME: Record<string, string> = Object.fromEntries(
  Object.entries(ALL_EVM_CHAIN_IDS).map(([name, id]) => [id, name]),
);

// ============================================================================
// Chain detection helpers
// ============================================================================

/** Returns true if the token is Bitcoin on Lightning. */
export function isLightning(token: { chain: string }): boolean {
  return token.chain.toLowerCase() === "lightning";
}

/** Returns true if the token is Bitcoin on Arkade. */
export function isArkade(token: { chain: string }): boolean {
  return token.chain.toLowerCase() === "arkade";
}

/** Returns true if the token is Bitcoin on-chain (L1). */
export function isBtcOnchain(token: { chain: string }): boolean {
  return token.chain.toLowerCase() === "bitcoin";
}

/** Returns true if the token is any form of Bitcoin (Lightning, Arkade, or on-chain). */
export function isBtc(token: { chain: string }): boolean {
  return isLightning(token) || isArkade(token) || isBtcOnchain(token);
}

/**
 * Returns true if the token is a BTC-pegged EVM token (WBTC or tBTC).
 * These tokens should be displayed like BTC (sats/BTC, 8 decimal precision)
 * even though tBTC has 18 on-chain decimals.
 */
export function isBtcPegged(token: { chain: string; symbol: string }): boolean {
  const sym = token.symbol.toLowerCase();
  return (sym === "wbtc" || sym === "tbtc") && isEvmToken(token.chain);
}

/** Returns true if the chain is any EVM chain (source or bridge destination). */
export function isEvmToken(chain: string): boolean {
  return (
    SOURCE_EVM_CHAINS.includes(chain as (typeof SOURCE_EVM_CHAINS)[number]) ||
    Object.values(ALL_EVM_CHAIN_IDS).includes(chain)
  );
}

/** Returns true if the chain is a source EVM chain (has HTLC contracts). */
export function isSourceEvmChain(chain: string): boolean {
  return SOURCE_EVM_CHAINS.includes(
    chain as (typeof SOURCE_EVM_CHAINS)[number],
  );
}

/** Returns true if the chain is a bridge-only destination (no HTLC contracts). */
export function isBridgeOnlyChain(chain: string): boolean {
  if (isSolanaToken(chain)) return true;
  return isEvmToken(chain) && !isSourceEvmChain(chain);
}

/**
 * Returns true if the chain is Solana. Solana is a CCTP-only destination —
 * funds reach it via Circle's Forwarding Service after an Arbitrum-side
 * burn, never as a swap source/target chain in its own right.
 */
export function isSolanaToken(chain: string): boolean {
  return chain.toLowerCase() === "solana";
}

/**
 * Returns true if the token is USDC on a CCTPv2-supported chain.
 *
 * Used to decide whether a token can act as the *source* of a swap via
 * bridge-kit (any CCTP chain → Arbitrum → BTC): we bridge the user's USDC
 * into Arbitrum first, then run the existing Arbitrum USDC → BTC swap.
 */
export function isCctpUsdc(token: {
  chain: string;
  symbol: string;
  token_id?: string;
}): boolean {
  if (token.symbol.toUpperCase() !== "USDC") return false;
  const tokenChainLower = token.chain.toLowerCase();
  for (const chainName of Object.keys(CCTP_DOMAINS)) {
    if (tokenChainLower === chainName.toLowerCase()) return true;
    const chainId = ALL_EVM_CHAIN_IDS[chainName];
    if (chainId && token.chain === chainId) return true;
  }
  return false;
}

/** Returns true if the chain is Ethereum. */
export function isEthereumToken(c: string): boolean {
  return c.toLowerCase() === "ethereum" || c === "1";
}

/** Returns true if the chain is Polygon. */
export function isPolygonToken(c: string): boolean {
  return c.toLowerCase() === "polygon" || c === "137";
}

/** Returns true if the chain is Arbitrum. */
export function isArbitrumToken(chain: string): boolean {
  return chain.toLowerCase() === "arbitrum" || chain === "42161";
}

export function isBaseToken(chain: string): boolean {
  return chain.toLowerCase() === "base" || chain === "8453";
}

export function isOptimismToken(chain: string): boolean {
  return chain.toLowerCase() === "optimism" || chain === "10";
}

export function isAvalancheToken(chain: string): boolean {
  return chain.toLowerCase() === "avalanche" || chain === "43114";
}

export function isLineaToken(chain: string): boolean {
  return chain.toLowerCase() === "linea" || chain === "59144";
}

export function isSonicToken(chain: string): boolean {
  return chain.toLowerCase() === "sonic" || chain === "146";
}

/** Normalizes any chain string to its canonical Chain value. */
export function toChain(str: string): Chain {
  const c = str.toLowerCase();
  if (c === "ethereum" || c === "1") return "1";
  if (c === "polygon" || c === "137") return "137";
  if (c === "arbitrum" || c === "42161") return "42161";
  if (c === "lightning") return "Lightning";
  if (c === "arkade") return "Arkade";
  if (c === "bitcoin") return "Bitcoin";
  // Solana is a CCTP destination only — not a first-class Chain in the
  // backend's union, so we cast through. Frontends use the literal "Solana"
  // string for icons / dropdown filters.
  if (c === "solana") return "Solana" as Chain;
  // Check new chains by name
  for (const [name, id] of Object.entries(ALL_EVM_CHAIN_IDS)) {
    if (c === name.toLowerCase() || c === id) return id as Chain;
  }
  return "Bitcoin";
}

export function toChainName(chain: Chain): string {
  return CHAIN_ID_TO_NAME[chain] ?? chain.toString();
}

/**
 * Get the CCTP bridge target chain name for a token, if bridging is needed.
 * Returns undefined if the token is on a source chain (no bridge needed)
 * or is not an EVM token.
 */
export function getBridgeTargetChain(token: TokenInfo): string | undefined {
  if (isBridgeOnlyChain(token.chain)) {
    return CHAIN_ID_TO_NAME[token.chain] ?? toChainName(token.chain as Chain);
  }
  return undefined;
}

// ============================================================================
// CCTP USDC bridge tokens (target-only)
// ============================================================================

/**
 * Generate TokenInfo objects for USDC on all CCTP-supported chains.
 * These are "bridge-only" target tokens — the swap runs on a source chain
 * (Polygon/Ethereum/Arbitrum) and USDC is bridged to the destination via CCTP.
 *
 * Excludes source chains (those already have USDC tokens from the backend).
 */
export function getCctpBridgeTokens(): TokenInfo[] {
  const sourceChainNames = new Set(["Ethereum", "Polygon", "Arbitrum"]);
  const tokens: TokenInfo[] = [];

  for (const chainName of Object.keys(CCTP_DOMAINS)) {
    // Skip source chains (backend already provides their USDC tokens)
    if (sourceChainNames.has(chainName)) continue;

    const usdcAddress = USDC_ADDRESSES[chainName];
    if (!usdcAddress) continue;

    // Solana has no EVM chain id — use the literal "Solana" string as the
    // chain identifier (matches the backend's `bridge_target_chain` value).
    const chainIdentifier =
      chainName === "Solana" ? "Solana" : ALL_EVM_CHAIN_IDS[chainName];
    if (!chainIdentifier) continue;

    tokens.push({
      token_id: usdcAddress as TokenId,
      symbol: "USDC",
      name: `USD Coin (${chainName})`,
      decimals: 6,
      chain: chainIdentifier as Chain,
    });
  }

  return tokens;
}

// ============================================================================
// USDT0 OFT bridge tokens (target-only)
// ============================================================================

/**
 * Generate TokenInfo objects for USDT0 on all LayerZero OFT-supported chains.
 * These are "bridge-only" target tokens — the swap runs on Arbitrum and USDT0
 * is bridged to the destination via LayerZero OFT send().
 *
 * Excludes source chains (those already have USDT0 tokens from the backend).
 */
export function getUsdt0BridgeTokens(): TokenInfo[] {
  const sourceChainNames = new Set(["Ethereum", "Polygon", "Arbitrum"]);
  const tokens: TokenInfo[] = [];

  for (const chainName of Object.keys(LZ_EIDS)) {
    // Skip source chains (backend already provides their USDT tokens)
    if (sourceChainNames.has(chainName)) continue;

    const usdt0Address = USDT0_ADDRESSES[chainName];
    const chainId = ALL_EVM_CHAIN_IDS[chainName];
    if (!usdt0Address || !chainId) continue;

    tokens.push({
      token_id: usdt0Address as TokenId,
      symbol: "USDT",
      name: `Tether USD (${chainName})`,
      decimals: 6,
      chain: chainId as Chain,
    });
  }

  return tokens;
}
