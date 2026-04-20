/**
 * Types for swap creation operations.
 */

import type {
  BitcoinToEvmSwapResponse as ApiBitcoinToEvmSwapResponse,
  ApiClient,
  EvmToBitcoinSwapResponse as ApiEvmToBitcoinSwapResponse,
  ArkadeToEvmSwapResponse,
  ArkadeToLightningSwapResponse,
  BtcToArkadeSwapResponse,
  EvmToArkadeSwapResponse,
  EvmToLightningSwapResponse,
  LightningToArkadeSwapResponse,
  LightningToEvmSwapResponse,
  TokenId,
  TokenInfo,
} from "../api/client.js";
import type { SwapParams } from "../signer/index.js";
import type { Asset } from "../tokens.js";

// Placeholder types until OpenAPI spec is regenerated
// These match the Rust API response types

/** Supported EVM chains for swaps */
export type EvmChain = "polygon" | "arbitrum" | "ethereum" | string;

/** Parameters for CCTP USDC bridging after a DEX swap. */
export interface UsdcBridgeParams {
  /** CCTP destination chain name (e.g., "Base", "Ethereum"). */
  targetChain: string;
  /** Native USDC contract address on the destination chain. */
  targetTokenAddress: string;
}

/**
 * Parameters describing a CCTP inbound bridge on an EVM→* swap.
 *
 * Set when the user's source USDC originates on a CCTP chain that the
 * backend doesn't operate natively on (Optimism, Base, Linea, …). The
 * swap is created against Arbitrum-native USDC; the SDK surfaces this
 * metadata so the backend can account for the CCTPv2 fast-transfer fee
 * that's deducted on the burn.
 */
export interface UsdcInboundBridgeParams {
  /** CCTP source chain name (e.g., "Optimism", "Base"). */
  sourceChain: string;
  /** Native USDC contract address on the source chain. */
  sourceTokenAddress: string;
}

/** Options for creating an Arkade or Lightning to EVM swap */
export interface BtcToEvmSwapOptions {
  /** Target EVM address to receive tokens */
  targetAddress: string;
  /** Target token ID (e.g., "usdc_pol", "usdt_arb") */
  targetToken: TokenId;
  /** Target EVM chain */
  targetChain: EvmChain;
  /** Amount in satoshis to send (optional if targetAmount is set) */
  sourceAmount?: number;
  /** Amount of target token to receive (optional if sourceAmount is set) */
  targetAmount?: number;
  /** Optional referral code for fee exemption */
  referralCode?: string;
}

/** Options for creating a Bitcoin (on-chain) to EVM swap via the generic endpoint */
export interface BitcoinToEvmSwapOptions {
  /** EVM address where tokens are swept after the claim (user's final destination) */
  targetAddress: string;
  /** ERC-20 contract address of the desired token on the target chain */
  tokenAddress: string;
  /** Numeric EVM chain ID: 1 (Ethereum), 137 (Polygon), 42161 (Arbitrum) */
  evmChainId: number;
  /** Amount in satoshis to send (mutually exclusive with targetAmount) */
  sourceAmount?: number;
  /** Amount of target token to receive in smallest unit (mutually exclusive with sourceAmount) */
  targetAmount?: number;
  /** Optional referral code for fee exemption */
  referralCode?: string;
  /** Whether the server should execute the DEX swap on behalf of the user (gasless claim). Defaults to true. */
  gasless?: boolean;
  /** Optional: when set, USDC is bridged via CCTP to the destination chain after the DEX swap. */
  bridgeParams?: UsdcBridgeParams;
}

/** Response from the generic `/swap/bitcoin/evm` endpoint. */
export type BitcoinToEvmSwapResponse = ApiBitcoinToEvmSwapResponse;

/** Result of creating a Bitcoin (on-chain) to EVM swap */
export interface BitcoinToEvmSwapResult {
  /** The swap response from the API */
  response: BitcoinToEvmSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/** Options for creating a Bitcoin (on-chain) to Arkade swap */
export interface BitcoinToArkadeSwapOptions {
  /** Amount in satoshis to receive on Arkade */
  satsReceive: number;
  /** Target Arkade address to receive VTXOs */
  targetAddress: string;
  /** Optional referral code for fee exemption */
  referralCode?: string;
}

/** Result of creating a Bitcoin (on-chain) to Arkade swap */
export interface BitcoinToArkadeSwapResult {
  /** The swap response from the API */
  response: BtcToArkadeSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/** Options for creating an EVM to Arkade swap */
export interface EvmToArkadeSwapOptions {
  /** Source EVM chain */
  sourceChain: EvmChain;
  /** Source token ID (e.g., "usdc_pol", "usdt_arb", "usdc_eth") */
  sourceToken: string;
  /** Amount of source token to send */
  sourceAmount: number;
  /** Target Arkade address to receive BTC */
  targetAddress: string;
  /** User's EVM wallet address (for checking allowance and building transactions) */
  userAddress: string;
  /** Optional referral code for fee exemption */
  referralCode?: string;
}

/** Result of creating an EVM to Arkade swap */
export interface EvmToArkadeSwapResult {
  /** The swap response from the API */
  response: EvmToArkadeSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/** Options for creating an EVM to Lightning swap (chain-specific - deprecated) */
export interface EvmToLightningSwapOptions {
  /** Source EVM chain */
  sourceChain: EvmChain;
  /** Source token ID (e.g., "usdc_pol", "usdt_arb", "usdc_eth") */
  sourceToken: string;
  /** Lightning BOLT11 invoice to pay */
  bolt11Invoice: string;
  /** User's EVM wallet address (for checking allowance and building transactions) */
  userAddress: string;
  /** Optional referral code for fee exemption */
  referralCode?: string;
}

/** Options for creating a Lightning-to-EVM swap via the generic endpoint */
export interface LightningToEvmSwapGenericOptions {
  /** EVM address where tokens are swept after the claim (user's final destination) */
  targetAddress: string;
  /** Numeric EVM chain ID: 1 (Ethereum), 137 (Polygon), 42161 (Arbitrum) */
  evmChainId: number;
  /** ERC-20 contract address of the desired token on the target chain */
  tokenAddress: string;
  /** Amount in satoshis to send (mutually exclusive with amountOut) */
  amountIn?: number;
  /** Amount of target token to receive in smallest unit (mutually exclusive with amountIn) */
  amountOut?: number;
  /** Optional referral code */
  referralCode?: string;
  /** Whether the server should execute the DEX swap on behalf of the user (gasless claim). Defaults to true. */
  gasless?: boolean;
  /** Optional: when set, USDC is bridged via CCTP to the destination chain after the DEX swap. */
  bridgeParams?: UsdcBridgeParams;
}

/** Result of creating a Lightning-to-EVM swap via the generic endpoint */
export interface LightningToEvmSwapGenericResult {
  /** The swap response from the API */
  response: LightningToEvmSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/** Options for creating an EVM-to-Lightning swap via the generic endpoint.
 *
 * Provide **one of** `lightningInvoice`, `lightningAddress` + `amountSats`, or `lnurl` + `amountSats`.
 */
export interface EvmToLightningSwapGenericOptions {
  /** User's BOLT11 Lightning invoice. Mutually exclusive with `lightningAddress` and `lnurl`. */
  lightningInvoice?: string;
  /** Lightning address (e.g. `user@speed.app`). Mutually exclusive with `lightningInvoice` and `lnurl`. Requires `amountSats`. */
  lightningAddress?: string;
  /** Raw LNURL string (e.g. `lnurl1...`). Mutually exclusive with `lightningInvoice` and `lightningAddress`. Requires `amountSats`. */
  lnurl?: string;
  /** Amount in satoshis the recipient should receive. Required when `lightningAddress` or `lnurl` is provided. */
  amountSats?: number;
  /** Numeric EVM chain ID: 1 (Ethereum), 137 (Polygon), 42161 (Arbitrum) */
  evmChainId: number;
  /** ERC-20 contract address of the source token on the EVM chain */
  tokenAddress: string;
  /** User's EVM address (sender of the ERC-20 token) */
  userAddress: string;
  /** Optional referral code */
  referralCode?: string;
  /** Use gasless relay. When true, userAddress is auto-derived from the swap's secretKey. */
  gasless?: boolean;
  /** Optional: when set, source USDC originates on another CCTP chain and hops to Arbitrum via CCTPv2. */
  inboundBridgeParams?: UsdcInboundBridgeParams;
}

/** Result of creating an EVM-to-Lightning swap via the generic endpoint */
export interface EvmToLightningSwapGenericResult {
  /** The swap response from the API */
  response: EvmToLightningSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/** Options for creating an EVM-to-Arkade swap via the generic endpoint */
export interface EvmToArkadeSwapGenericOptions {
  /** Target Arkade address to receive BTC */
  targetAddress: string;
  /** ERC-20 contract address of the source token on the EVM chain */
  tokenAddress: string;
  /** Numeric EVM chain ID: 1 (Ethereum), 137 (Polygon), 42161 (Arbitrum) */
  evmChainId: number;
  /** User's EVM wallet address (sender of the ERC-20 token) */
  userAddress: string;
  /** Amount of source token to send in smallest units (mutually exclusive with targetAmount) */
  sourceAmount?: bigint;
  /** Desired BTC output in sats (mutually exclusive with sourceAmount) */
  targetAmount?: number;
  /** Optional referral code */
  referralCode?: string;
  /** Use gasless relay. When true, userAddress is auto-derived from the swap's secretKey. */
  gasless?: boolean;
  /** Optional: when set, source USDC originates on another CCTP chain and hops to Arbitrum via CCTPv2. */
  inboundBridgeParams?: UsdcInboundBridgeParams;
}

/** Result of creating an EVM-to-Arkade swap via the generic endpoint */
export interface EvmToArkadeSwapGenericResult {
  /** The swap response from the API */
  response: EvmToArkadeSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/** Options for creating an EVM-to-Bitcoin (on-chain) swap via the generic endpoint */
export interface EvmToBitcoinSwapOptions {
  /** ERC-20 contract address of the source token on the EVM chain */
  tokenAddress: string;
  /** Numeric EVM chain ID: 1 (Ethereum), 137 (Polygon), 42161 (Arbitrum) */
  evmChainId: number;
  /** User's EVM wallet address (sender of the ERC-20 token) */
  userAddress: string;
  /** User's BTC address to receive claimed funds */
  targetAddress: string;
  /** Amount of source token to send in smallest units (mutually exclusive with targetAmount) */
  sourceAmount?: bigint;
  /** Desired BTC output in sats (mutually exclusive with sourceAmount) */
  targetAmount?: number;
  /** Optional referral code */
  referralCode?: string;
  /** Use gasless relay. When true, userAddress is auto-derived from the swap's secretKey. */
  gasless?: boolean;
  /** Optional: when set, source USDC originates on another CCTP chain and hops to Arbitrum via CCTPv2. */
  inboundBridgeParams?: UsdcInboundBridgeParams;
}

/** Result of creating an EVM-to-Bitcoin (on-chain) swap */
export interface EvmToBitcoinSwapResult {
  /** The swap response from the API */
  response: ApiEvmToBitcoinSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/** Options for creating an Arkade-to-EVM swap via the generic endpoint */
export interface ArkadeToEvmSwapOptions {
  /**
   * EVM address where tokens are swept after the claim (user's final destination).
   * This is required and will be stored on the server for use during redemption.
   */
  targetAddress: string;
  /** ERC-20 contract address of the desired token on the target chain */
  tokenAddress: string;
  /** Numeric EVM chain ID: 1 (Ethereum), 137 (Polygon), 42161 (Arbitrum) */
  evmChainId: number;
  /** Amount in satoshis to send (mutually exclusive with targetAmount) */
  sourceAmount?: bigint;
  /** Amount of target token to receive in smallest unit (mutually exclusive with sourceAmount) */
  targetAmount?: bigint;
  /** Optional referral code */
  referralCode?: string;
  /** Whether the server should execute the DEX swap on behalf of the user (gasless claim). Defaults to true. */
  gasless?: boolean;
  /** Optional: when set, USDC is bridged via CCTP to the destination chain after the DEX swap. */
  bridgeParams?: UsdcBridgeParams;
}

/** Result of creating an Arkade-to-EVM swap via the generic endpoint */
export interface ArkadeToEvmSwapResult {
  /** The swap response from the API */
  response: ArkadeToEvmSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/**
 * Options for the generic `createSwap` method that routes to the correct direction.
 *
 * Accepts either:
 * - `source` / `target` — simplified {@link Asset} identifiers (preferred)
 * - `sourceAsset` / `targetAsset` — full `TokenInfo` objects (legacy, still supported)
 *
 * ```ts
 * // Preferred: use Asset constants
 * await client.createSwap({
 *   source: Asset.BTC_ARKADE,
 *   target: Asset.USDC_POLYGON,
 *   sourceAmount: 100_000,
 *   targetAddress: "0x...",
 * });
 *
 * // Also works: any chain + tokenId
 * await client.createSwap({
 *   source: { chain: "Arkade", tokenId: "btc" },
 *   target: { chain: "137", tokenId: "0x3c499c..." },
 *   sourceAmount: 100_000,
 *   targetAddress: "0x...",
 * });
 * ```
 */
export interface CreateSwapOptions {
  /** Source asset (preferred). Takes priority over `sourceAsset`. */
  source?: Asset;
  /** Target asset (preferred). Takes priority over `targetAsset`. */
  target?: Asset;
  /** @deprecated Use `source` instead. Full TokenInfo object. */
  sourceAsset?: TokenInfo;
  /** @deprecated Use `target` instead. Full TokenInfo object. */
  targetAsset?: TokenInfo;
  sourceAmount?: number;
  targetAmount?: number;
  /** Target address: EVM address, Arkade address, or Lightning invoice */
  targetAddress: string;
  /** EVM address of the user (required for EVM→* swaps) */
  userAddress?: string;
  referralCode?: string;
  /** Whether to use gasless relay for EVM funding (no wallet needed). When true, the SDK-derived EVM address is used as the depositor. */
  gasless?: boolean;
  /** Optional: when set, USDC is bridged via CCTP to the destination chain after the DEX swap. */
  bridgeParams?: UsdcBridgeParams;
  /** Optional: when set, source USDC originates on another CCTP chain and hops to Arbitrum via CCTPv2. Auto-populated when the source chain is CCTP-only. */
  inboundBridgeParams?: UsdcInboundBridgeParams;
}

/** Options for creating a Lightning-to-Arkade swap */
export interface LightningToArkadeSwapOptions {
  /** Amount in satoshis to receive on Arkade */
  satsReceive: number;
  /** Target Arkade address to receive VTXOs */
  targetAddress: string;
  /** Optional referral code for fee exemption */
  referralCode?: string;
}

/** Result of creating a Lightning-to-Arkade swap */
export interface LightningToArkadeSwapResult {
  /** The swap response from the API */
  response: LightningToArkadeSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/** Options for creating an Arkade-to-Lightning swap.
 *
 * Provide **one of** `lightningInvoice`, `lightningAddress` + `amountSats`, or `lnurl` + `amountSats`.
 */
export interface ArkadeToLightningSwapOptions {
  /** User's BOLT11 Lightning invoice. Mutually exclusive with `lightningAddress` and `lnurl`. */
  lightningInvoice?: string;
  /** Lightning address (e.g. `user@speed.app`). Mutually exclusive with `lightningInvoice` and `lnurl`. Requires `amountSats`. */
  lightningAddress?: string;
  /** Raw LNURL string (e.g. `lnurl1...`). Mutually exclusive with `lightningInvoice` and `lightningAddress`. Requires `amountSats`. */
  lnurl?: string;
  /** Amount in satoshis the recipient should receive. Required when `lightningAddress` or `lnurl` is provided. */
  amountSats?: number;
  /** Optional referral code for fee tracking */
  referralCode?: string;
}

/** Result of creating an Arkade-to-Lightning swap */
export interface ArkadeToLightningSwapResult {
  /** The swap response from the API */
  response: ArkadeToLightningSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/** Union of all swap creation results returned by `createSwap`. */
export type CreateSwapResult =
  | ArkadeToEvmSwapResult
  | ArkadeToLightningSwapResult
  | BitcoinToEvmSwapResult
  | BitcoinToArkadeSwapResult
  | LightningToEvmSwapGenericResult
  | LightningToArkadeSwapResult
  | EvmToArkadeSwapGenericResult
  | EvmToBitcoinSwapResult
  | EvmToLightningSwapGenericResult;

/**
 * Context passed to swap creation functions.
 * Contains the dependencies needed from the client.
 */
export interface CreateSwapContext {
  /** The API client for making requests */
  apiClient: ApiClient;
  /** The base URL for the API (for endpoints not yet in OpenAPI spec) */
  baseUrl: string;
  /** Function to derive swap parameters (auto-increments key index) */
  deriveSwapParams: () => Promise<SwapParams>;
  /** Deterministic EVM address derived from the SDK key, reused across all swaps for Permit2 efficiency. */
  evmAddress: string;
  /** Jump the key index forward by `n` (used to skip past reused indices). */
  skipKeyIndices?: (n: number) => Promise<void>;
  /** Function to store the swap in storage (if configured) */
  storeSwap: (
    swapId: string,
    swapParams: SwapParams,
    response: Record<string, unknown>,
    targetAddress?: string,
  ) => Promise<void>;
}
