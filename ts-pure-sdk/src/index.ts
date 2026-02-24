export const VERSION = "0.0.1";

// API types
export type {
  ApiClient,
  ApiClientOptions,
  ArkadeToEvmSwapRequest,
  ArkadeToEvmSwapResponse,
  BitcoinToEvmSwapRequest,
  BtcToArkadeSwapResponse,
  Chain,
  ClaimGaslessRequest,
  ClaimGaslessResponse,
  components,
  EvmToArkadeSwapResponse,
  EvmToBitcoinSwapRequest,
  EvmToBitcoinSwapResponse,
  EvmToLightningSwapResponse,
  GetSwapResponse,
  LightningToEvmSwapResponse,
  // Types
  paths,
  QuoteResponse,
  SwapStatus,
  TokenId,
  TokenInfo,
  TokenInfos,
} from "./api/client.js";
// API client
export { createApiClient } from "./api/client.js";
// Arkade VHTLC query utilities
export {
  type GetVhtlcAmountsParams,
  getVhtlcAmounts,
  type VhtlcAmounts,
  type VtxoStatus,
} from "./arkade.js";
export type {
  ArkadeClaimOptions,
  BitcoinToArkadeSwapOptions,
  BitcoinToArkadeSwapResult,
  BitcoinToEvmSwapOptions,
  BitcoinToEvmSwapResponse,
  BitcoinToEvmSwapResult,
  BtcToEvmSwapOptions,
  ClaimGaslessResult,
  ClaimOptions,
  ClaimResult,
  ClientConfig,
  CoordinatorFundingCallData,
  CreateSwapOptions,
  CreateSwapResult,
  EthereumClaimData,
  EvmChain,
  EvmFundingCallData,
  EvmToArkadeSwapGenericOptions,
  EvmToArkadeSwapGenericResult,
  EvmToArkadeSwapOptions,
  EvmToArkadeSwapResult,
  EvmToBitcoinSwapOptions,
  EvmToBitcoinSwapResult,
  EvmToLightningSwapOptions,
  OnchainRefundOptions,
  RefundOptions,
  RefundResult,
} from "./client.js";
// Main client
export { Client, ClientBuilder } from "./client.js";
// EVM HTLC utilities
export {
  type ApproveCallData,
  buildEvmHtlcCallData,
  type CreateSwapCallData,
  type CreateSwapParams,
  deriveEvmAddress,
  encodeApproveCallData,
  encodeCreateSwapCallData,
  encodeHtlcErc20CreateCallData,
  type HtlcErc20CreateCallData,
  type HtlcErc20CreateParams,
  signEvmDigest,
} from "./evm/index.js";
export {
  calculateSourceAmount,
  calculateTargetAmount,
  computeExchangeRate,
} from "./price-calculations.js";
// Redeem module (Arkade claim)
export {
  type ArkadeClaimParams,
  type ArkadeClaimResult,
  buildArkadeClaim,
} from "./redeem/index.js";
// Refund module
export {
  type BitcoinNetwork,
  buildOnchainClaimTransaction,
  buildOnchainRefundTransaction,
  computeHash160,
  type OnchainClaimParams,
  type OnchainClaimResult,
  type OnchainRefundParams,
  type OnchainRefundResult,
  verifyHtlcAddress,
} from "./refund/index.js";
export type { SwapParams } from "./signer/index.js";
// Signer (HD wallet key derivation)
export { bytesToHex, hexToBytes, Signer } from "./signer/index.js";
// IndexedDB storage (browser)
export {
  IdbSwapStorage,
  IdbWalletStorage,
  idbStorageFactory,
} from "./storage/idb.js";
export type {
  StorageFactory,
  StoredSwap,
  SwapStorage,
  WalletStorage,
} from "./storage/index.js";
// Storage interfaces and implementations
export {
  InMemorySwapStorage,
  InMemoryWalletStorage,
  inMemoryStorageFactory,
  SWAP_STORAGE_VERSION,
} from "./storage/index.js";
// Token helpers and constants
export {
  BTC_ARKADE,
  BTC_ARKADE_INFO,
  BTC_LIGHTNING,
  BTC_LIGHTNING_INFO,
  BTC_ONCHAIN,
  BTC_ONCHAIN_INFO,
  isArbitrumToken,
  isArkade,
  isBtc,
  isBtcOnchain,
  isEthereumToken,
  isEvmToken,
  isLightning,
  isPolygonToken,
  type TokenInput,
  toChain,
  toChainName,
} from "./tokens.js";
export { getUsdPrices } from "./usd-price.js";
