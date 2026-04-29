/**
 * CCTP (Cross-Chain Transfer Protocol) constants.
 *
 * Uses CCTP V2 contracts which share the same address across all EVM chains.
 * See: https://developers.circle.com/cctp/references/contract-addresses
 */

// ============================================================================
// CCTP V2 contract addresses (same on all EVM chains)
// ============================================================================

export const TOKEN_MESSENGER_V2 = "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d";
export const MESSAGE_TRANSMITTER_V2 =
  "0x81D40F21F12A8F0E3252Bccb954D722d4c464B64";

// ============================================================================
// CCTP Domain IDs
// See: https://developers.circle.com/cctp/concepts/supported-chains-and-domains
// ============================================================================

export const CCTP_DOMAINS = {
  Ethereum: 0,
  Avalanche: 1,
  Optimism: 2,
  Arbitrum: 3,
  Solana: 5,
  Base: 6,
  Polygon: 7,
  Unichain: 10,
  Linea: 11,
  Sonic: 13,
  "World Chain": 14,
  Monad: 15,
  Sei: 16,
  HyperEVM: 19,
  Ink: 21,
} as const;

export type CctpChainName = keyof typeof CCTP_DOMAINS;

// ============================================================================
// USDC addresses per chain (native USDC)
// ============================================================================

export const USDC_ADDRESSES: Record<string, string> = {
  Ethereum: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
  Polygon: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
  Arbitrum: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
  Base: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
  Optimism: "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85",
  Avalanche: "0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E",
  Linea: "0x176211869ca2b568f2a7d4ee941e073a821ee1ff",
  Unichain: "0x078d782b760474a361dda0af3839290b0ef57ad6",
  "World Chain": "0x79A02482A880bCe3F13E09da970dC34dB4cD24D1",
  Ink: "0x2D270e6886d130D724215A266106e6832161EAEd",
  Sonic: "0x29219dd400f2bf60e5a23d13be72b486d4038894",
  Sei: "0xe15fC38F6D8c56aF07bbCBe3BAf5708A2Bf42392",
  HyperEVM: "0xb88339CB7199b77E23DB6E890353E22632Ba630f",
  Monad: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
  // Solana's USDC is an SPL mint, encoded as a base58 pubkey rather than
  // an EVM `0x...` hex address. Callers that parse the value with viem's
  // `getAddress` / `Address.from_str` must guard against this entry.
  Solana: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
};

export const EURC_ADDRESSES: Record<string, string> = {
  Ethereum: "0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c",
};

export const USAT_ADDRESSES: Record<string, string> = {
  Ethereum: "0x07041776F5007aCa2A54844f50503a18A72A8b68",
};

// ============================================================================
// V2 addresses are the same on all chains, but kept as Records for
// backward compatibility and for the receiveMessage destination lookup.
// ============================================================================

export const TOKEN_MESSENGER_ADDRESSES: Record<string, string> =
  Object.fromEntries(
    Object.keys(CCTP_DOMAINS).map((chain) => [chain, TOKEN_MESSENGER_V2]),
  );

export const MESSAGE_TRANSMITTER_ADDRESSES: Record<string, string> =
  Object.fromEntries(
    Object.keys(CCTP_DOMAINS).map((chain) => [chain, MESSAGE_TRANSMITTER_V2]),
  );

// ============================================================================
// Forwarding Service
// ============================================================================

/**
 * Magic hookData for Circle's Forwarding Service.
 * Encodes "cctp-forward" + version 0 + data length 0.
 * When passed to depositForBurnWithHook, Circle auto-mints on destination — zero gas needed.
 */
export const FORWARDING_SERVICE_HOOK_DATA =
  "0x636374702d666f72776172640000000000000000000000000000000000000000";

/** Fast transfer finality threshold. */
export const FINALITY_FAST = 1000;
/** Standard transfer finality threshold. */
export const FINALITY_STANDARD = 2000;

/** Forwarding fee for Ethereum destination ($1.25 in USDC units). */
export const FORWARDING_FEE_ETHEREUM = 1_250_000n;
/** Forwarding fee for all other destinations ($0.20 in USDC units). */
export const FORWARDING_FEE_OTHER = 200_000n;

// ============================================================================
// Circle Attestation (IRIS) API
// ============================================================================

export const IRIS_API_MAINNET = "https://iris-api.circle.com";
export const IRIS_API_TESTNET = "https://iris-api-sandbox.circle.com";
