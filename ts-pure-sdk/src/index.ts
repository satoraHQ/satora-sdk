export const VERSION = "0.0.1";

// API types
export type {
  ApiClient,
  ApiClientOptions,
  ArkadeToEvmSwapRequest,
  ArkadeToEvmSwapResponse,
  ArkadeToLightningSwapResponse,
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
  LightningToArkadeSwapResponse,
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
// CCTP (Cross-Chain Transfer Protocol) utilities
export {
  type AttestationResponse,
  type AttestationResult,
  type AttestationStatus,
  addressToBytes32,
  type BridgeParams,
  type BurnResult,
  bytes32ToAddress,
  CCTP_DOMAINS,
  type CctpChainName,
  type FetchAttestationOptions,
  FINALITY_FAST,
  FINALITY_STANDARD,
  FORWARDING_FEE_ETHEREUM,
  FORWARDING_FEE_OTHER,
  FORWARDING_SERVICE_HOOK_DATA,
  fetchAttestation,
  getDomain,
  IRIS_API_MAINNET,
  IRIS_API_TESTNET,
  MESSAGE_TRANSMITTER_ADDRESSES,
  MESSAGE_TRANSMITTER_V2,
  type MintResult,
  needsBridge,
  TOKEN_MESSENGER_ADDRESSES,
  TOKEN_MESSENGER_V2,
  USDC_ADDRESSES,
} from "./cctp/index.js";
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
  SupportAgentInfo,
  UsdcBridgeParams,
} from "./client.js";
// Main client
export { Client, ClientBuilder } from "./client.js";
// EVM HTLC utilities
export {
  type ApproveCallData,
  buildEvmHtlcCallData,
  buildPermit2TypedData,
  type CoordinatorCall,
  type CreateSwapCallData,
  type CreateSwapParams,
  deriveEvmAddress,
  type EvmSigner,
  type ExecuteAndCreateWithPermit2Params,
  encodeApproveCallData,
  encodeCreateSwapCallData,
  encodeExecuteAndCreateWithPermit2,
  encodeHtlcErc20CreateCallData,
  type HtlcErc20CreateCallData,
  type HtlcErc20CreateParams,
  isUserRejection,
  PERMIT2_ADDRESS,
  type Permit2TypedData,
  signEvmDigest,
  type TxReceipt,
  type UnsignedPermit2FundingData,
} from "./evm/index.js";
export {
  calculateSourceAmount,
  calculateTargetAmount,
  computeExchangeRate,
} from "./price-calculations";
// Escrow signing utilities
export {
  type SignedEscrowTx,
  getArkTxid,
  signEscrowArkTx,
  signEscrowCheckpoints,
} from "./escrow/index.js";
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
  Asset,
  BTC_ARKADE,
  BTC_ARKADE_INFO,
  BTC_LIGHTNING,
  BTC_LIGHTNING_INFO,
  BTC_ONCHAIN,
  BTC_ONCHAIN_INFO,
  getBridgeTargetChain,
  getCctpBridgeTokens,
  isArbitrumToken,
  isArkade,
  isAvalancheToken,
  isBaseToken,
  isBridgeOnlyChain,
  isBtc,
  isBtcOnchain,
  isBtcPegged,
  isEthereumToken,
  isEvmToken,
  isLightning,
  isLineaToken,
  isOptimismToken,
  isPolygonToken,
  isSourceEvmChain,
  type TokenInput,
  toChain,
  toChainName,
} from "./tokens.js";
export { getUsdPrices } from "./usd-price.js";
