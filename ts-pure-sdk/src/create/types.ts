/**
 * Types for swap creation operations.
 */

import type {
  ApiClient,
  BtcToArkadeSwapResponse,
  BtcToEvmSwapResponse,
  EvmToBtcSwapResponse,
  GetSwapResponse,
  OnchainToEvmSwapResponse,
  TokenId,
} from "../api/client.js";
import type { SwapParams } from "../signer/index.js";

/** Supported EVM chains for swaps */
export type EvmChain = "polygon" | "arbitrum" | "ethereum" | string;

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

/** Options for creating a Bitcoin (on-chain) to EVM swap */
export interface BitcoinToEvmSwapOptions {
  /** Target EVM address to receive tokens */
  targetAddress: string;
  /** Target token ID (e.g., "usdc_pol", "usdt_arb") */
  targetToken: TokenId;
  /** Target EVM chain */
  targetChain: EvmChain;
  /** Amount in satoshis to send */
  sourceAmount: number;
  /** Optional referral code for fee exemption */
  referralCode?: string;
}

/** Result of creating a BTC to EVM swap */
export interface BtcToEvmSwapResult {
  /** The swap response from the API */
  response: BtcToEvmSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/**
 * Union type for Bitcoin on-chain swap responses.
 * Note: The API returns different types for different chains due to spec inconsistency.
 * All chains actually return OnchainToEvmSwapResponse in practice.
 */
export type BitcoinToEvmSwapResponse =
  | BtcToEvmSwapResponse
  | OnchainToEvmSwapResponse;

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
  response: EvmToBtcSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/** Options for creating an EVM to Lightning swap */
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

/** Result of creating an EVM to Lightning swap */
export interface EvmToLightningSwapResult {
  /** The swap response from the API */
  response: EvmToBtcSwapResponse;
  /** The swap parameters used (for storage/recovery) */
  swapParams: SwapParams;
}

/**
 * Context passed to swap creation functions.
 * Contains the dependencies needed from the client.
 */
export interface CreateSwapContext {
  /** The API client for making requests */
  apiClient: ApiClient;
  /** Function to derive swap parameters (auto-increments key index) */
  deriveSwapParams: () => Promise<SwapParams>;
  /** Function to store the swap in storage (if configured) */
  storeSwap: (
    swapId: string,
    swapParams: SwapParams,
    response: GetSwapResponse,
  ) => Promise<void>;
}
