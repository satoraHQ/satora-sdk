import {
  type ApiClient,
  type ArkadeToEvmSwapResponse,
  type ArkadeToLightningSwapResponse,
  type BtcToArkadeSwapResponse,
  type Chain,
  createApiClient,
  type EvmToArkadeSwapResponse,
  type EvmToBitcoinSwapResponse,
  type EvmToLightningSwapResponse,
  type GetSwapResponse,
  type LightningToArkadeSwapResponse,
  type LightningToEvmSwapResponse,
  type QuoteResponse,
  type SwapPairsResponse,
  type TokenInfos,
} from "./api/client.js";
import { getVhtlcAmounts, type VhtlcAmounts } from "./arkade.js";
import { USDC_ADDRESSES } from "./cctp/constants.js";
import { computeCctpFastFee, getCachedCctpFee } from "./cctp/fee.js";
import {
  cctpMetaForChainId,
  isCctpOnlySource,
} from "./cctp-inbound/chainMap.js";
import { CctpInboundClient } from "./cctp-inbound/client.js";
import type {
  CctpFundSwapResult,
  CctpProgressStep,
} from "./cctp-inbound/fundSwap.js";
import type { AaConfig } from "./cctp-inbound/types.js";

import {
  type ArkadeToEvmSwapOptions,
  type ArkadeToEvmSwapResult,
  type ArkadeToLightningSwapOptions,
  type ArkadeToLightningSwapResult,
  type BitcoinToArkadeSwapOptions,
  type BitcoinToArkadeSwapResult,
  type BitcoinToEvmSwapOptions,
  type BitcoinToEvmSwapResult,
  type CreateSwapContext,
  type CreateSwapOptions,
  type CreateSwapResult,
  createArkadeToEvmSwapGeneric,
  createArkadeToLightningSwap,
  createBitcoinToArkadeSwap,
  createBitcoinToEvmSwap,
  createEvmToArkadeSwapGeneric,
  createEvmToBitcoinSwap,
  createEvmToLightningSwapGeneric,
  createLightningToArkadeSwap,
  createLightningToEvmSwapGeneric,
  type EvmToArkadeSwapGenericOptions,
  type EvmToArkadeSwapGenericResult,
  type EvmToBitcoinSwapOptions,
  type EvmToBitcoinSwapResult,
  type EvmToLightningSwapGenericOptions,
  type EvmToLightningSwapGenericResult,
  type LightningToArkadeSwapOptions,
  type LightningToArkadeSwapResult,
  type LightningToEvmSwapGenericOptions,
  type LightningToEvmSwapGenericResult,
} from "./create/index.js";
import { delegateClaim, delegateRefund } from "./delegate.js";
import { broadcastTransaction, findOutputByAddress } from "./esplora.js";
import {
  buildCollabRefundEvmDigest,
  buildCollabRefundEvmTypedData,
  buildEip2612PermitDigest,
  buildPermit2FundingDigest,
  buildPermit2TypedData,
  type CollabRefundEvmDigestParams,
  type CollabRefundEvmTypedData,
  deriveEvmAddress,
  encodeApproveCallData,
  encodeExecuteAndCreateWithPermit2,
  encodeHtlcErc20RefundCallData,
  PERMIT2_ADDRESS,
  type Permit2SignedFundingCallData,
  signEvmDigest,
  type UnsignedPermit2FundingData,
} from "./evm/index.js";
import {
  decodeUint256,
  type EvmSigner,
  encodeAllowanceCall,
  encodeBalanceOfCall,
  encodeMaxApproveData,
  getRevertReason,
  parseSignature,
  simulateTransaction,
} from "./evm/wallet.js";
import {
  buildArkadeClaim,
  type ClaimGaslessResult,
  type ClaimResult,
  continueArkadeClaim,
  claimViaGasless as gaslessClaim,
  claim as redeemClaim,
} from "./redeem/index.js";
import {
  type BitcoinNetwork,
  buildArkadeRefund,
  buildOnchainClaimTransaction,
  buildOnchainRefundTransaction,
  collabRefundArkadeToEvmDelegate,
  collabRefundArkadeToEvmOffchain,
  collabRefundArkadeToLightningOffchain,
  verifyHtlcAddress,
} from "./refund/index.js";
import {
  bytesToHex,
  hexToBytes,
  Signer,
  type SwapParams,
} from "./signer/index.js";
import {
  type StoredSwap,
  SWAP_STORAGE_VERSION,
  type SwapStorage,
  type WalletStorage,
} from "./storage/index.js";
import {
  isArkade,
  isBridgeOnlyChain,
  isBtcOnchain,
  isBtcPegged,
  isEvmToken,
  isLightning,
  isSourceEvmChain,
  toChainName,
} from "./tokens.js";
import { USDT0_ADDRESSES } from "./usdt0-bridge/constants.js";
import {
  createSwapStatusWatcher,
  type SwapStatusHandler,
  type SwapStatusWatcher,
} from "./ws.js";

// Re-export types from create module for backwards compatibility
export type {
  ArkadeToEvmSwapOptions,
  ArkadeToEvmSwapResult,
  BitcoinToArkadeSwapOptions,
  BitcoinToArkadeSwapResult,
  BitcoinToEvmSwapOptions,
  BitcoinToEvmSwapResponse,
  BitcoinToEvmSwapResult,
  BtcToEvmSwapOptions,
  CreateSwapOptions,
  CreateSwapResult,
  EvmChain,
  EvmToArkadeSwapGenericOptions,
  EvmToArkadeSwapGenericResult,
  EvmToArkadeSwapOptions,
  EvmToArkadeSwapResult,
  EvmToBitcoinSwapOptions,
  EvmToBitcoinSwapResult,
  EvmToLightningSwapOptions,
  UsdcBridgeParams,
} from "./create/index.js";

import type { BitcoinToEvmSwapResponse } from "./create/index.js";

// Re-export coordinator utilities for Arkade-to-EVM redeemAndExecute flow
export {
  buildCollabRefundEvmDigest,
  buildCollabRefundEvmTypedData,
  buildEip2612PermitDigest,
  buildPermit2FundingDigest,
  buildRedeemCalls,
  buildRedeemDigest,
  type CollabRefundEvmDigestParams,
  type CollabRefundEvmTypedData,
  type CoordinatorCall,
  type Eip2612PermitParams,
  type ExecuteAndCreateWithPermit2Params,
  encodeExecuteAndCreateWithPermit2,
  encodeRedeemAndExecute,
  encodeRefundAndExecute,
  encodeRefundTo,
  PERMIT2_ADDRESS,
  type Permit2FundingParams,
  type Permit2SignedFundingCallData,
  type RedeemAndExecuteCallData,
  type RedeemAndExecuteParams,
  type RedeemDigestParams,
  type RefundAndExecuteParams,
  type RefundToParams,
} from "./evm/index.js";
// Re-export types from redeem module
export type {
  ClaimGaslessResult,
  ClaimResult,
  CoordinatorClaimData,
  EthereumClaimData,
} from "./redeem/index.js";

/** A support agent's Nostr identity */
export interface SupportAgentInfo {
  npub: string;
}

/** Result of attempting a refund */
export interface RefundResult {
  /** Whether the refund was successful */
  success: boolean;
  /** Human-readable message about the refund status */
  message: string;
  /** Raw transaction hex (for on-chain refunds) */
  txHex?: string;
  /** Transaction ID (for on-chain refunds) */
  txId?: string;
  /** Amount being refunded in satoshis (after fees) */
  refundAmount?: bigint;
  /** Fee paid in satoshis */
  fee?: bigint;
  /** Whether the transaction was broadcast to the network */
  broadcast?: boolean;
  /** The HTLC address we computed locally (for debugging) */
  htlcAddress?: string;
  /** The HTLC address reported by the server (for debugging) */
  serverHtlcAddress?: string;
  /** EVM refund data (for evm_to_arkade and evm_to_btc swaps) */
  evmRefundData?: {
    /** Address to send the refund transaction to (coordinator or HTLC) */
    to: string;
    /** ABI-encoded calldata for the refund call */
    data: string;
    /** Whether the timelock has already expired (refund is available) */
    timelockExpired: boolean;
    /** Unix timestamp when the timelock expires */
    timelockExpiry: number;
  };
}

/** Options for on-chain refund */
export interface OnchainRefundOptions {
  /** Destination address to receive refunded BTC */
  destinationAddress: string;
  /** Fee rate in satoshis per virtual byte (default: 2) */
  feeRateSatPerVb?: number;
  /** If true, only build the transaction without broadcasting (default: false) */
  dryRun?: boolean;
}

/** Options for Arkade (off-chain) refund */
export interface ArkadeRefundOptions {
  /** Destination Arkade address to receive refunded BTC */
  destinationAddress: string;
  /** Arkade server URL (optional, uses default based on network) */
  arkadeServerUrl?: string;
}

/** Options for EVM refund via coordinator */
export interface EvmRefundOptions {
  /**
   * Settlement mode — what asset you receive:
   * - "swap-back" (default): Swap WBTC back to original token via DEX
   * - "direct": Return WBTC directly
   */
  mode?: "swap-back" | "direct";
  /**
   * Whether to use collaborative refund (server cosigns + submits, gasless, no timelock wait).
   * When false/undefined, the refund requires timelock expiry and the caller submits the tx.
   * @default false
   */
  collaborative?: boolean;
}

/** Result of a collaborative EVM refund */
export interface CollabRefundEvmResult {
  /** Swap ID */
  id: string;
  /** On-chain transaction hash */
  txHash: string;
  /** Success message */
  message: string;
}

/** Parameters for building CollabRefund EIP-712 typed data (returned by getCollabRefundEvmParams) */
export interface CollabRefundEvmParams {
  /** HTLCCoordinator contract address (EIP-712 verifyingContract) */
  coordinatorAddress: string;
  /** Server's signer EOA address (the `caller` field in the EIP-712 struct) */
  serverSignerAddress: string;
  /** Preimage hash (0x-prefixed, 32-byte hex) */
  preimageHash: string;
  /** WBTC amount locked in the HTLC (decimal string) */
  amount: string;
  /** WBTC token address */
  token: string;
  /** Claim address (server's EVM address) */
  claimAddress: string;
  /** HTLC timelock (unix timestamp) */
  timelock: number;
  /** EVM chain ID */
  chainId: number;
  /** Settlement mode: "direct" or "swap-back" */
  mode: string;
  /** Token the depositor receives (WBTC for direct, source token for swap-back) — the EIP-712 `sweepToken` field */
  sweepToken: string;
  /** Minimum output amount for the sweep — the EIP-712 `minAmountOut` field */
  minAmountOut: string;
  /** keccak256(abi.encode(calls)) for the exact calls array signed in CollabRefund */
  callsHash: string;
  /** Source token address (only present for swap-back) */
  sourceTokenAddress?: string;
  /** DEX calldata for swap-back (only present when mode=swap-back) */
  dexCalldata?: { to: string; data: string; value: string };
}

/** General refund options — the method picks the right variant based on swap type */
export type RefundOptions =
  | OnchainRefundOptions
  | ArkadeRefundOptions
  | EvmRefundOptions;

/** Options for Arkade (off-chain) claim */
export interface ArkadeClaimOptions {
  /** Destination Arkade address to receive claimed BTC */
  destinationAddress: string;
  /** Arkade server URL (optional, uses default based on network) */
  arkadeServerUrl?: string;
  /**
   * If the VTXO has not been indexed yet (status `not_funded`), keep
   * polling for up to this many milliseconds before giving up. The
   * server-funded WS update can race ahead of the Arkade indexer; a
   * short wait absorbs that lag without surfacing a transient error.
   * Set to `0` to fail immediately. Default: 30_000.
   */
  waitForVtxoMs?: number;
}

/** Options for claiming a swap */
export interface ClaimOptions {
  /**
   * @deprecated For Arkade-to-EVM swaps, the destination is now set at swap creation time
   * and stored on the server. This option is ignored for arkade_to_evm swaps.
   */
  destination?: string;
  /** Bitcoin destination address for EVM-to-Bitcoin claims (required for evm_to_bitcoin direction) */
  destinationAddress?: string;
  /** Fee rate in sat/vB for on-chain Bitcoin claims (default: 2) */
  feeRateSatPerVb?: number;
}

/** Result of getting EVM funding call data */
export interface EvmFundingCallData {
  /** Call data for approving token spend (ERC20 approve) */
  approve: {
    /** Token contract address to call */
    to: string;
    /** Encoded approve(spender, amount) call data */
    data: string;
  };
  /** Call data for creating the swap (from server) */
  createSwap: {
    /** HTLC contract address to call */
    to: string;
    /** Encoded createSwap call data (from server) */
    data: string;
  };
}

/** Result of getting coordinator refund call data */
export interface CoordinatorRefundCallData {
  /** Contract address to call */
  to: string;
  /** Encoded refund call data */
  data: string;
  /** Whether the timelock has expired (refund is possible) */
  timelockExpired: boolean;
  /** Unix timestamp when the timelock expires */
  timelockExpiry: number;
  /** Refund mode used */
  mode: "swap-back" | "direct";
}

/** Internal type for VHTLC claim/refund parameters extracted from a stored swap. */
interface ArkadeVhtlcParams {
  userSecretKey: string;
  userPubKey: string;
  lendaswapPubKey: string;
  arkadeServerPubKey: string;
  vhtlcAddress: string;
  refundLocktime: number;
  unilateralClaimDelay: number;
  unilateralRefundDelay: number;
  unilateralRefundWithoutReceiverDelay: number;
  network: string;
  preimage: string;
  preimageHash: string;
}

const DEFAULT_BASE_URL = "https://api.lendaswap.com/";

/** Default Esplora URLs by network */
const DEFAULT_ESPLORA_URLS: Record<string, string> = {
  mainnet: "https://mempool.space/api",
  signet: "https://mutinynet.com/api",
  regtest: "http://localhost:3000",
};

/** Configuration options for the Lendaswap client. */
export interface ClientConfig {
  /** The base URL of the Lendaswap API. */
  baseUrl: string;
  /** Optional unique identifier for the organization using the SDK. */
  orgCode?: string;
  /** Optional default headers to send with SDK API requests. */
  defaultHeaders?: Record<string, string>;
  /** Optional Esplora API URL for broadcasting Bitcoin transactions. */
  esploraUrl?: string;
  /** Optional Arkade server URL (e.g. "https://arkade.computer"). Falls back to network-based defaults. */
  arkadeServerUrl?: string;
  /**
   * Optional account-abstraction config (bundler + Gas Manager policy).
   * Required when using the CCTP-inbound flow; a clear error is thrown
   * on first CCTP API call if omitted.
   */
  aa?: AaConfig;
}

/**
 * Builder for creating a Lendaswap client with a fluent API.
 *
 * The `build()` method is async and returns a fully initialized client.
 *
 * @example
 * ```ts
 * // Create client with new wallet (generates mnemonic)
 * const client = await Client.builder()
 *   .withSignerStorage(new IdbWalletStorage())
 *   .build();
 *
 * // Create client with existing mnemonic
 * const client = await Client.builder()
 *   .withSignerStorage(new IdbWalletStorage())
 *   .withMnemonic("abandon abandon abandon ...")
 *   .build();
 *
 * // Create client from a BIP32 extended private key (ephemeral — never persisted).
 * // Useful when the secret lives in an env var, KMS, or external secure store.
 * const client = await Client.builder()
 *   .withXprv("xprv9s21ZrQH143K...")
 *   .build();
 *
 * // Create client without storage (stateless, generates new mnemonic)
 * const client = await Client.builder().build();
 * ```
 */
export class ClientBuilder {
  #baseUrl: string = DEFAULT_BASE_URL;
  #orgCode?: string;
  #defaultHeaders?: Record<string, string>;
  #esploraUrl?: string;
  #arkadeServerUrl?: string;
  #signerStorage?: WalletStorage;
  #swapStorage?: SwapStorage;
  #mnemonic?: string;
  #xprv?: string;
  #aa?: AaConfig;

  /**
   * Sets the base URL for the API.
   * @param baseUrl - The base URL of the Lendaswap API.
   * @returns The builder instance for chaining.
   */
  withBaseUrl(baseUrl: string): this {
    this.#baseUrl = baseUrl;
    return this;
  }

  /**
   * Sets the org code.
   * @param orgCode - The identifier for the organization sending the request.
   * @returns The builder instance for chaining.
   */
  withOrgCode(orgCode: string): this {
    this.#orgCode = orgCode;
    return this;
  }

  /**
   * Sets default headers to send with SDK API requests.
   * @param headers - Headers merged into API requests made by the SDK.
   * @returns The builder instance for chaining.
   */
  withDefaultHeaders(headers: Record<string, string>): this {
    this.#defaultHeaders = {
      ...this.#defaultHeaders,
      ...headers,
    };
    return this;
  }

  /**
   * Sets the Esplora API URL for broadcasting Bitcoin transactions.
   *
   * If not set, defaults will be used based on the network:
   * - mainnet: https://mempool.space/api
   * - testnet: https://mempool.space/testnet/api
   * - signet: https://mempool.space/signet/api
   *
   * @param esploraUrl - The Esplora API base URL.
   * @returns The builder instance for chaining.
   */
  withEsploraUrl(esploraUrl: string): this {
    this.#esploraUrl = esploraUrl;
    return this;
  }

  /**
   * Sets the Arkade server URL for VHTLC operations (claim, refund, amounts).
   *
   * If not set, defaults are used based on the network:
   * - bitcoin: https://arkade.computer
   * - signet: wa
   *
   * @param arkadeServerUrl - The Arkade server base URL.
   * @returns The builder instance for chaining.
   */
  withArkadeServerUrl(arkadeServerUrl: string): this {
    this.#arkadeServerUrl = arkadeServerUrl;
    return this;
  }

  /**
   * Sets the storage backend for signer data (mnemonic and key index).
   * @param storage - The storage implementation to use.
   * @returns The builder instance for chaining.
   */
  withSignerStorage(storage: WalletStorage): this {
    this.#signerStorage = storage;
    return this;
  }

  /**
   * Sets the storage backend for swap data.
   *
   * When configured, swaps will be automatically persisted after creation.
   *
   * @param storage - The swap storage implementation to use.
   * @returns The builder instance for chaining.
   */
  withSwapStorage(storage: SwapStorage): this {
    this.#swapStorage = storage;
    return this;
  }

  /**
   * Sets the mnemonic phrase to use for the signer.
   *
   * If provided, this mnemonic will be used instead of loading from storage
   * or generating a new one. The mnemonic will be persisted to storage if
   * storage is configured.
   *
   * @param mnemonic - The BIP39 mnemonic phrase (12, 15, 18, 21, or 24 words).
   * @returns The builder instance for chaining.
   */
  withMnemonic(mnemonic: string): this {
    this.#mnemonic = mnemonic;
    return this;
  }

  /**
   * Sets a BIP32 extended private key (xprv) to use for the signer.
   *
   * The xprv is treated as ephemeral: it is **never** persisted to signer
   * storage, even if storage is configured. The caller is responsible for
   * supplying it on every `build()` (e.g. from an env var, KMS, or vault).
   * Storage, if configured, is still used for the swap key index counter.
   *
   * Cannot be combined with `withMnemonic()`.
   *
   * @param xprv - The base58check-encoded extended private key.
   * @returns The builder instance for chaining.
   * @throws Error if `xprv` is empty or whitespace-only — fail fast on a
   *         misconfigured secret rather than silently falling back to storage.
   */
  withXprv(xprv: string): this {
    if (typeof xprv !== "string" || xprv.trim().length === 0) {
      throw new Error("withXprv() requires a non-empty xprv string");
    }
    this.#xprv = xprv;
    return this;
  }

  /**
   * Sets the account-abstraction config (bundler + Gas Manager policy).
   *
   * Required when using the CCTP-inbound swap flow (any non-Arbitrum
   * EVM chain as the source). The settlement UserOp — `receiveMessage`
   * + `USDC.approve(Permit2)` + `executeAndCreateWithPermit2` — is
   * submitted via the Kernel smart account owned by the consumer's
   * connected wallet, sponsored by the Gas Manager so users need no
   * ETH on Arbitrum.
   *
   * @param aa - Bundler URL + paymaster policy id.
   * @returns The builder instance for chaining.
   */
  withAa(aa: AaConfig): this {
    this.#aa = aa;
    return this;
  }

  /**
   * Builds and returns a fully initialized Client instance.
   *
   * Initialization order:
   * 1. If `withXprv()` was called, use that xprv (ephemeral, never stored)
   * 2. Else if `withMnemonic()` was called, use that mnemonic
   * 3. Else if storage is configured and contains a mnemonic, load it
   * 4. Else generate a new mnemonic
   *
   * The mnemonic is persisted to storage if storage is configured. An xprv
   * is never persisted.
   *
   * @returns A promise that resolves to a fully initialized Client.
   * @throws Error if the provided mnemonic or xprv is invalid, or if both
   *         `withMnemonic()` and `withXprv()` were called.
   */
  async build(): Promise<Client> {
    if (this.#xprv && this.#mnemonic) {
      throw new Error(
        "withMnemonic() and withXprv() are mutually exclusive — pick one",
      );
    }

    let signer: Signer;

    if (this.#xprv) {
      // Ephemeral xprv — never touch signer storage for the secret
      signer = Signer.fromXprv(this.#xprv);
    } else if (this.#mnemonic) {
      // Use provided mnemonic
      signer = Signer.fromMnemonic(this.#mnemonic);
      if (this.#signerStorage) {
        await this.#signerStorage.setMnemonic(this.#mnemonic);
      }
    } else if (this.#signerStorage) {
      // Try to load from storage
      const storedMnemonic = await this.#signerStorage.getMnemonic();
      if (storedMnemonic) {
        signer = Signer.fromMnemonic(storedMnemonic);
      } else {
        // Generate new and persist
        signer = Signer.generate();
        // Signer.generate() always populates mnemonic
        await this.#signerStorage.setMnemonic(signer.mnemonic as string);
      }
    } else {
      // No storage, generate new (stateless mode)
      signer = Signer.generate();
    }

    return new Client(
      {
        baseUrl: this.#baseUrl.replace(/\/+$/, ""),
        orgCode: this.#orgCode,
        defaultHeaders: this.#defaultHeaders,
        esploraUrl: this.#esploraUrl?.replace(/\/+$/, ""),
        arkadeServerUrl: this.#arkadeServerUrl?.replace(/\/+$/, ""),
        aa: this.#aa,
      },
      signer,
      this.#signerStorage,
      this.#swapStorage,
    );
  }
}

/**
 * Main client for interacting with the Lendaswap API.
 *
 * The client manages:
 * - API communication
 * - Signer (HD wallet) for key derivation
 * - Storage for persisting mnemonic and key index
 *
 * Use `Client.builder()` to create a new instance.
 *
 * @example
 * ```ts
 * const client = await Client.builder()
 *   .withSignerStorage(new IdbWalletStorage())
 *   .withOrgCode("your-org-code")
 *   .build();
 *
 * // Get mnemonic (for backup)
 * const mnemonic = client.getMnemonic();
 *
 * // Derive swap parameters
 * const params = await client.deriveSwapParams();
 * ```
 */
export class Client {
  readonly #apiClient: ApiClient;
  readonly #config: ClientConfig;
  #signer: Signer;
  readonly #signerStorage?: WalletStorage;
  readonly #swapStorage?: SwapStorage;
  #statusWatcher: SwapStatusWatcher | null = null;
  #cctpInbound: CctpInboundClient | null = null;

  /**
   * Creates a new Client instance.
   *
   * Use `Client.builder()` instead of calling this constructor directly.
   *
   * @internal
   */
  constructor(
    config: ClientConfig,
    signer: Signer,
    signerStorage?: WalletStorage,
    swapStorage?: SwapStorage,
  ) {
    this.#config = config;
    this.#apiClient = createApiClient({
      baseUrl: config.baseUrl,
      orgCode: config.orgCode,
      defaultHeaders: config.defaultHeaders,
    });
    this.#signer = signer;
    this.#signerStorage = signerStorage;
    this.#swapStorage = swapStorage;
  }

  /**
   * Subscribe to status updates for one or more swaps over a shared WebSocket.
   * The socket is opened lazily on first call and closed automatically when
   * the last subscriber unsubscribes.
   *
   * @param swapIds Swap ids to watch.
   * @param onUpdate Fires with `(swapId, status)` for every status change of
   *                 any id in the set.
   * @returns Unsubscribe function that removes `onUpdate` from all of the
   *          passed ids at once.
   */
  subscribeToSwaps(swapIds: string[], onUpdate: SwapStatusHandler): () => void {
    if (!this.#statusWatcher) {
      this.#statusWatcher = createSwapStatusWatcher(this.#config.baseUrl);
    }
    return this.#statusWatcher.subscribe(swapIds, onUpdate);
  }

  /**
   * Remove `onUpdate` from the given swap ids.
   */
  unsubscribeFromSwaps(swapIds: string[], onUpdate: SwapStatusHandler): void {
    this.#statusWatcher?.unsubscribe(swapIds, onUpdate);
  }

  /** Force-close the shared swap-status socket. */
  closeSwapStatusSocket(): void {
    this.#statusWatcher?.close();
    this.#statusWatcher = null;
  }

  /**
   * Creates a new ClientBuilder for fluent configuration.
   * @returns A new ClientBuilder instance.
   */
  static builder(): ClientBuilder {
    return new ClientBuilder();
  }

  /** The underlying typed API client for direct API access. */
  get api(): ApiClient {
    return this.#apiClient;
  }

  /** The base URL of the API. */
  get baseUrl(): string {
    return this.#config.baseUrl;
  }

  /**
   * Namespace for CCTP-inbound swap primitives (source-chain burn,
   * IRIS attestation, settlement UserOp). Requires `withAa(...)` on
   * the builder — throws with a clear error otherwise.
   *
   * For simple integrations prefer `Client.fundSwap(swapId, signer)`
   * which auto-dispatches to the CCTP path when needed; drop down to
   * `client.cctpInbound.*` when you need step-by-step progress control.
   */
  get cctpInbound(): CctpInboundClient {
    if (!this.#cctpInbound) {
      if (!this.#config.aa) {
        throw new Error(
          "CCTP-inbound flow requires AA config. Call `.withAa({ bundlerUrl, paymasterPolicyId })` on the ClientBuilder before `.build()`.",
        );
      }
      this.#cctpInbound = new CctpInboundClient({
        apiClient: this.#apiClient,
        aa: this.#config.aa,
      });
    }
    return this.#cctpInbound;
  }

  /** The swap storage, if configured. */
  get swapStorage(): SwapStorage | undefined {
    return this.#swapStorage;
  }

  // =========================================================================
  // Signer Methods
  // =========================================================================

  /**
   * Gets the mnemonic phrase.
   *
   * Store this securely - it's the only way to recover the wallet.
   *
   * @returns The BIP39 mnemonic phrase.
   * @throws Error if the wallet was initialized from an xprv (no mnemonic exists).
   */
  getMnemonic(): string {
    const mnemonic = this.#signer.mnemonic;
    if (!mnemonic) {
      throw new Error(
        "No mnemonic available — wallet was initialized from an xprv",
      );
    }
    return mnemonic;
  }

  /**
   * Loads a mnemonic phrase, replacing the current signer.
   *
   * The new mnemonic is persisted to storage if storage is configured.
   *
   * @param mnemonic - The BIP39 mnemonic phrase to load.
   * @throws Error if the mnemonic is invalid.
   */
  async loadMnemonic(mnemonic: string): Promise<void> {
    this.#signer = Signer.fromMnemonic(mnemonic);
    if (this.#signerStorage) {
      await this.#signerStorage.setMnemonic(mnemonic);
    }
  }

  /**
   * Derives a deterministic Nostr private key from the wallet mnemonic.
   *
   * Uses the NIP-06 derivation path so the same mnemonic always produces
   * the same Nostr identity.
   *
   * @returns The 32-byte Nostr private key as a hex string.
   */
  getNostrKeyHex(): string {
    return this.#signer.deriveNostrKeyHex();
  }

  /**
   * Gets the user ID extended public key for wallet recovery.
   *
   * This can be shared with the server for recovering swap history.
   *
   * @returns The hex-encoded user ID xpub.
   */
  getUserIdXpub(): string {
    return this.#signer.getUserIdXpubString();
  }

  /**
   * Derives swap parameters at the next available index.
   *
   * Automatically increments the key index in storage (if configured).
   *
   * @returns The derived swap parameters.
   */
  async deriveSwapParams(): Promise<SwapParams> {
    let index = 0;
    if (this.#signerStorage) {
      index = await this.#signerStorage.incrementKeyIndex();
    }
    return this.#signer.deriveSwapParams(index);
  }

  /**
   * Derives swap parameters at a specific index.
   *
   * Does not modify the stored key index. Useful for recovery scenarios.
   *
   * @param index - The key index to derive.
   * @returns The derived swap parameters.
   */
  deriveSwapParamsAtIndex(index: number): SwapParams {
    return this.#signer.deriveSwapParams(index);
  }

  /**
   * Get the deterministic EVM address for this SDK key.
   *
   * This address is reused across all gasless swaps, so a single
   * Permit2 approval is sufficient for multiple swaps.
   *
   * @returns Checksummed EVM address (0x-prefixed).
   */
  getEvmAddress(): string {
    const { secretKey } = this.#signer.deriveEvmKey();
    return deriveEvmAddress(secretKey);
  }

  /**
   * Get the EVM signing key, derived deterministically from the mnemonic.
   *
   * @internal
   */
  #getEvmSigningKey(): string {
    return bytesToHex(this.#signer.deriveEvmKey().secretKey);
  }

  /**
   * Gets the current key index from storage.
   * @returns The current key index, or 0 if no storage is configured.
   */
  async getKeyIndex(): Promise<number> {
    if (this.#signerStorage) {
      return this.#signerStorage.getKeyIndex();
    }
    return 0;
  }

  /**
   * Sets the key index in storage.
   *
   * Useful for recovery scenarios where you need to set the index
   * to a specific value.
   *
   * @param index - The new key index.
   * @throws Error if no storage is configured.
   */
  async setKeyIndex(index: number): Promise<void> {
    if (!this.#signerStorage) {
      throw new Error("No signer storage configured");
    }
    await this.#signerStorage.setKeyIndex(index);
  }

  // =========================================================================
  // Health & Info
  // =========================================================================

  /**
   * Checks the health status of the API.
   * @returns A promise that resolves to "ok" if the API is healthy.
   * @throws Error if the health check fails.
   */
  async healthCheck(): Promise<string> {
    const { data, error } = await this.#apiClient.GET("/health");
    if (error) {
      throw new Error(`Health check failed: ${JSON.stringify(error)}`);
    }
    return data ?? "ok";
  }

  /**
   * Gets the version information of the API.
   * @returns A promise that resolves to the version info containing tag and commit hash.
   * @throws Error if the request fails.
   */
  async getVersion(): Promise<{ tag: string; commit_hash: string }> {
    const { data, error } = await this.#apiClient.GET("/version");
    if (error) {
      throw new Error(`Failed to get version: ${JSON.stringify(error)}`);
    }
    if (!data) {
      throw new Error("No version data returned");
    }
    return data;
  }

  /**
   * Gets the list of support agent npubs from the backend config.
   * @returns A promise that resolves to an array of support agent info objects.
   * @throws Error if the request fails.
   */
  async getSupportAgents(): Promise<SupportAgentInfo[]> {
    const baseUrl = this.#config.baseUrl.replace(/\/$/, "");
    const resp = await fetch(`${baseUrl}/support-agents`);
    if (!resp.ok) {
      throw new Error(`Failed to get support agents: ${resp.status}`);
    }
    const data: { agents: SupportAgentInfo[] } = await resp.json();
    return data.agents;
  }

  /**
   * Gets the current Median Time Past (MTP) and tip block height.
   * @returns A promise that resolves to the MTP timestamp and tip height.
   * @throws Error if the request fails or MTP is not yet available.
   */
  async getMtp(): Promise<{ mtp: number; tip_height: number }> {
    const { data, error } = await this.#apiClient.GET("/mtp");
    if (error) {
      throw new Error(`Failed to get MTP: ${JSON.stringify(error)}`);
    }
    if (!data) {
      throw new Error("MTP not available yet");
    }
    return data;
  }

  // =========================================================================
  // Tokens & Asset Pairs
  // =========================================================================

  /**
   * Gets the list of supported tokens.
   * @returns A promise that resolves to an array of token information.
   * @throws Error if the request fails.
   */
  async getTokens(): Promise<TokenInfos> {
    const { data, error } = await this.#apiClient.GET("/tokens");
    if (error || !data) {
      throw new Error(`Failed to get tokens: ${JSON.stringify(error)}`);
    }
    return data;
  }

  /**
   * Gets all supported swap pairs with their limits (in satoshis) and base fee percentages.
   * @returns A promise that resolves to all swap pairs with limits and fees.
   * @throws Error if the request fails.
   */
  async getSwapPairs(): Promise<SwapPairsResponse> {
    const { data, error } = await this.#apiClient.GET("/swap-pairs");
    if (error || !data) {
      throw new Error(`Failed to get swap pairs: ${JSON.stringify(error)}`);
    }
    return data;
  }

  // =========================================================================
  // Quotes
  // =========================================================================

  /**
   * Gets a quote for swapping between two tokens.
   * @param params - Quote parameters.
   * @param params.sourceChain - Source blockchain (e.g., "Arkade", "Polygon").
   * @param params.sourceToken - Source token: contract address for EVM tokens, or "btc" for BTC.
   * @param params.targetChain - Target blockchain (e.g., "Polygon", "Lightning").
   * @param params.targetToken - Target token: contract address for EVM tokens, or "btc" for BTC.
   * @param params.sourceAmount - Amount in smallest unit of source token (mutually exclusive with targetAmount).
   * @param params.targetAmount - Amount in smallest unit of target token (mutually exclusive with sourceAmount).
   * @param params.referralCode - Optional referral code to apply referral pricing to the quote.
   * @returns A promise that resolves to the quote response with pricing details.
   * @throws Error if the request fails.
   */
  async getQuote(params: {
    sourceChain: Chain;
    sourceToken: string;
    targetChain: Chain;
    targetToken: string;
    sourceAmount?: number;
    targetAmount?: number;
    referralCode?: string;
  }): Promise<QuoteResponse> {
    // If the target is a bridge-only chain (e.g. USDC on Base), remap the
    // quote request to the token on Arbitrum (source chain the backend knows).
    // Pass bridge_target_chain so the backend includes the bridge fee.
    let targetChain = params.targetChain;
    let targetToken = params.targetToken;
    let bridgeTargetChain: string | undefined;
    if (isBridgeOnlyChain(targetChain)) {
      bridgeTargetChain = toChainName(targetChain);
      // Determine if this is a USDT0 or USDC bridge token by checking
      // if the target token matches a known USDT0 address on the destination.
      const isUsdt0 = Object.values(USDT0_ADDRESSES).some(
        (addr) => addr.toLowerCase() === params.targetToken.toLowerCase(),
      );
      // Remap to Arbitrum for the DEX quote (cheapest gas, good liquidity).
      targetChain = "42161" as Chain;
      targetToken = isUsdt0
        ? USDT0_ADDRESSES.Arbitrum
        : USDC_ADDRESSES.Arbitrum;
    }

    // CCTP-only source (USDC on Optimism / Base / Linea / …): the quote
    // endpoint expects the source chain/token the DEX actually runs on
    // (Arbitrum + native USDC), plus `bridge_source_chain` + address so
    // the backend can apply the CCTPv2 fast-transfer fee. All gross-vs-net
    // math happens server-side.
    let sourceChain = params.sourceChain;
    let sourceToken = params.sourceToken;
    let bridgeSourceChain: string | undefined;
    let bridgeSourceTokenAddress: string | undefined;
    const parsedSourceChainId = Number.parseInt(params.sourceChain, 10);
    if (
      !Number.isNaN(parsedSourceChainId) &&
      isCctpOnlySource(parsedSourceChainId)
    ) {
      const source = cctpMetaForChainId(parsedSourceChainId);
      if (params.sourceToken.toLowerCase() !== source.usdc.toLowerCase()) {
        throw new Error(
          `Quote on ${source.name} requires native USDC (${source.usdc}); got ${params.sourceToken}. Only USDC is bridgeable via CCTP.`,
        );
      }
      bridgeSourceChain = source.name;
      bridgeSourceTokenAddress = source.usdc;
      sourceChain = "42161" as Chain;
      sourceToken = USDC_ADDRESSES.Arbitrum;
    }

    const { data, error } = await this.#apiClient.GET("/quote", {
      params: {
        query: {
          source_chain: sourceChain,
          source_token: sourceToken,
          target_chain: targetChain,
          target_token: targetToken,
          source_amount: params.sourceAmount,
          target_amount: params.targetAmount,
          bridge_target_chain: bridgeTargetChain,
          bridge_source_chain: bridgeSourceChain,
          bridge_source_token_address: bridgeSourceTokenAddress,
          ref: params.referralCode,
        },
      },
    });
    if (error) {
      throw new Error(`Failed to get quote: ${JSON.stringify(error)}`);
    }
    if (!data) {
      throw new Error("No quote data returned");
    }

    return data;
  }

  // =========================================================================
  // Swap Status
  // =========================================================================

  /**
   * Gets the status and details of a swap by its ID.
   * @param id - The UUID of the swap.
   * @param options - Optional settings.
   * @param options.updateStorage - If true, updates the swap in storage after fetching.
   * @returns A promise that resolves to the swap details.
   * @throws Error if the request fails or swap is not found.
   */
  async getSwap(
    id: string,
    options?: { updateStorage?: boolean },
  ): Promise<GetSwapResponse> {
    const { data, error } = await this.#apiClient.GET("/swap/{id}", {
      params: { path: { id } },
    });
    if (error) {
      throw new Error(`Failed to get swap: ${JSON.stringify(error)}`);
    }
    if (!data) {
      throw new Error("No swap data returned");
    }

    if (options?.updateStorage && this.#swapStorage) {
      await this.#swapStorage.update(id, data);
    }

    return data;
  }

  /**
   * Gets a swap from local storage without making a server request.
   *
   * Use this when you need swap data but don't need the latest status
   * from the server. The stored swap includes the preimage, keys, and
   * the last known swap response.
   *
   * @param id - The UUID of the swap.
   * @returns The stored swap data, or null if not found.
   *
   * @example
   * ```ts
   * const stored = await client.getStoredSwap(swapId);
   * if (stored) {
   *   console.log("Target:", stored.response.target_token);
   *   console.log("Status:", stored.response.status);
   * }
   * ```
   */
  async getStoredSwap(id: string): Promise<StoredSwap | null> {
    if (!this.#swapStorage) {
      return null;
    }
    return this.#swapStorage.get(id);
  }

  /**
   * Gets all stored swaps from local storage.
   *
   * @returns Array of all stored swap data, or empty array if no storage is configured.
   */
  async listAllSwaps(): Promise<StoredSwap[]> {
    if (!this.#swapStorage) {
      return [];
    }
    return this.#swapStorage.getAll();
  }

  async deleteSwap(id: string): Promise<void> {
    if (!this.#swapStorage) {
      return;
    }
    await this.#swapStorage.delete(id);
  }

  async clearSwapStorage(): Promise<void> {
    if (!this.#swapStorage) {
      return;
    }
    await this.#swapStorage.clear();
  }

  /**
   * Recovers all swaps associated with the current wallet from the server.
   *
   * Sends the user's xpub to the server, which returns all swaps belonging
   * to that wallet. For each recovered swap, re-derives the keys using the
   * swap's derivation index and stores it locally.
   *
   * After recovery, the key index is set to `highest_index + 1` so that
   * new swaps don't reuse derivation indices.
   *
   * @returns The recovered swaps stored locally.
   */
  async recoverSwaps(): Promise<StoredSwap[]> {
    console.log(`Recovering ...`);
    const xpub = this.getUserIdXpub();
    console.log(`Recovering ${xpub}`);

    const { data, error } = await this.#apiClient.POST("/swap/recover", {
      body: { xpub },
    });
    if (error) {
      throw new Error(`Failed to recover swaps: ${JSON.stringify(error)}`);
    }
    if (!data) {
      throw new Error("No recovery data returned");
    }

    const storedSwaps: StoredSwap[] = [];
    console.log(`Recovered data ${JSON.stringify(data)}`);

    for (const recoveredSwap of data.swaps) {
      const { index, ...response } = recoveredSwap;
      const swapParams = this.deriveSwapParamsAtIndex(index);

      await this.#storeSwap(response.id, swapParams, response);

      const stored = await this.getStoredSwap(response.id);
      if (stored) {
        storedSwaps.push(stored);
      }
    }

    // Update key index so new swaps don't reuse indices
    if (data.highest_index >= 0) {
      await this.setKeyIndex(data.highest_index + 1);
    }

    return storedSwaps;
  }

  /**
   * Gets VHTLC amounts for an Arkade swap.
   *
   * Queries the Arkade indexer for spendable, spent, and recoverable balances
   * at the VHTLC address associated with a swap. Works for:
   * - BTC → EVM swaps where the source asset is Arkade
   * - EVM → BTC swaps where the target asset is Arkade
   *
   * Reads swap data from local storage (does not contact the server).
   *
   * @param id - The UUID of the swap.
   * @returns The VHTLC amounts in satoshis.
   */
  async amountsForSwap(id: string): Promise<VhtlcAmounts> {
    const stored = await this.getStoredSwap(id);
    if (!stored) {
      throw new Error(`Swap not found in local storage: ${id}`);
    }

    const swap = stored.response;

    if (
      swap.direction !== "btc_to_arkade" &&
      swap.direction !== "arkade_to_evm" &&
      swap.direction !== "arkade_to_lightning" &&
      swap.direction !== "evm_to_arkade" &&
      swap.direction !== "lightning_to_arkade"
    ) {
      throw new Error(
        `amountsForSwap only applies to VHTLC-based swaps, got ${swap.direction}`,
      );
    }

    // Get VHTLC address based on swap direction
    let vhtlcAddress: string | undefined;
    if (swap.direction === "btc_to_arkade") {
      vhtlcAddress = (swap as BtcToArkadeSwapResponse).arkade_vhtlc_address;
    } else if (swap.direction === "lightning_to_arkade") {
      vhtlcAddress = (swap as LightningToArkadeSwapResponse)
        .arkade_vhtlc_address;
    } else if (swap.direction === "arkade_to_lightning") {
      vhtlcAddress = (swap as ArkadeToLightningSwapResponse)
        .arkade_vhtlc_address;
    } else if (
      swap.direction === "arkade_to_evm" ||
      swap.direction === "evm_to_arkade"
    ) {
      vhtlcAddress = (
        swap as
          | (ArkadeToEvmSwapResponse & { direction: "arkade_to_evm" })
          | (EvmToArkadeSwapResponse & { direction: "evm_to_arkade" })
      ).btc_vhtlc_address;
    }

    if (!vhtlcAddress) {
      throw new Error("Swap does not have an Arkade VHTLC address");
    }

    return getVhtlcAmounts({
      vhtlcAddress,
      network: swap.network,
      arkadeServerUrl: this.#config.arkadeServerUrl,
    });
  }

  // =========================================================================
  // Redeem
  // =========================================================================

  /**
   * Claims a swap by revealing the preimage.
   *
   * Reads swap data and preimage from local storage. The claim method
   * depends on the swap direction and target chain:
   * - **Arkade/Lightning-to-EVM**: Gasless claim via server
   * - **Other EVM swaps**: Returns call data for manual claiming
   * - **Arkade**: Claims via Arkade protocol
   *
   * @param id - The UUID of the swap.
   * @param options - For Arkade/Lightning-to-EVM, destination is set at swap creation.
   * @returns A ClaimResult with the outcome.
   *
   * @example
   * ```ts
   * // Arkade-to-EVM (gasless via server, uses stored target address)
   * const result = await client.claim(swapId);
   *
   * // Other swap types
   * const result = await client.claim(swapId);
   * if (result.success) {
   *   console.log("Claim TX:", result.txHash);
   * }
   * ```
   */
  async claim(id: string, options?: ClaimOptions): Promise<ClaimResult> {
    // Check swap storage is configured
    if (!this.#swapStorage) {
      return {
        success: false,
        message:
          "Swap storage is not configured. Cannot retrieve swap data needed for claim.",
      };
    }

    // Get stored swap data (contains preimage, keys, and swap response)
    const storedSwap = await this.#swapStorage.get(id);
    if (!storedSwap) {
      return {
        success: false,
        message: `Swap ${id} not found in local storage. Cannot claim without stored data.`,
      };
    }

    const swap = storedSwap.response;
    const secret = storedSwap.preimage;

    // EVM-targeted swaps: use gasless claim via server (SDK signs internally)
    // The destination is always the stored target_evm_address (set at swap creation time)
    if (
      swap.direction === "arkade_to_evm" ||
      swap.direction === "lightning_to_evm" ||
      swap.direction === "bitcoin_to_evm"
    ) {
      const evmSwap = swap as (
        | ArkadeToEvmSwapResponse
        | LightningToEvmSwapResponse
        | BitcoinToEvmSwapResponse
      ) & {
        direction: string;
      };
      // Use the stored target address - this was set when the swap was created
      const destination =
        evmSwap.target_evm_address ?? evmSwap.client_evm_address;

      if (!destination) {
        return {
          success: false,
          message:
            "Gasless claim failed: no target address found. " +
            "This swap may have been created before target address storage was implemented.",
        };
      }
      const gaslessResult = await this.claimViaGasless(id, destination);
      return {
        success: true,
        message: gaslessResult.message,
        txHash: gaslessResult.txHash,
      };
    }

    // EVM-to-Bitcoin: user claims BTC from on-chain Taproot HTLC with preimage
    if (swap.direction === "evm_to_bitcoin") {
      return this.#claimOnchainBtc(id, options);
    }

    // Check if target is Arkade (handle both string "btc_arkade" and TokenInfo object)
    const isArkadeTarget = swap.target_token.chain === "Arkade";

    if (isArkadeTarget) {
      // Determine destination address based on swap direction
      let destinationAddress: string | undefined;

      if (swap.direction === "btc_to_arkade") {
        const btcToArkadeSwap = swap as BtcToArkadeSwapResponse & {
          direction: "btc_to_arkade";
        };
        destinationAddress = btcToArkadeSwap.target_arkade_address;
      } else if (
        swap.direction === "evm_to_arkade" ||
        swap.direction === "lightning_to_arkade"
      ) {
        // For evm_to_arkade swaps, check if we have target_arkade_address in stored response
        // Check if we have target_arkade_address in the stored response.
        const storedResponse = swap as { target_arkade_address?: string };
        if (storedResponse.target_arkade_address) {
          destinationAddress = storedResponse.target_arkade_address;
        } else {
          // Fetch from API to get the full response with target_arkade_address
          const freshSwap = await this.getSwap(id);
          const evmToArkadeSwap = freshSwap as {
            target_arkade_address: string;
          };
          destinationAddress = evmToArkadeSwap.target_arkade_address;
        }
      }

      if (!destinationAddress) {
        return {
          success: false,
          message:
            "No Arkade destination address found in swap. Use claimArkade() with explicit destinationAddress.",
        };
      }

      const arkadeResult = await this.claimArkade(id, { destinationAddress });

      // Convert to ClaimResult format
      return {
        success: arkadeResult.success,
        message: arkadeResult.message,
        chain: "arkade",
        txHash: arkadeResult.txId,
      };
    }

    // For EVM chains, use the existing claim logic
    return redeemClaim(id, secret, {
      apiClient: this.#apiClient,
      getSwap: () => Promise.resolve(swap),
    });
  }

  /**
   * Claims an Arkade-to-EVM swap gaslessly via the server.
   *
   * The SDK builds the EIP-712 digest, signs it with the swap's internally
   * derived EVM key, and sends the signature + secret to the server. The
   * server submits the `coordinator.redeemAndExecute` transaction.
   *
   * @param id - The UUID of the swap.
   * @param destination - The EVM address where tokens should be sent.
   * @param slippage - Maximum acceptable slippage percentage for the DEX swap (e.g. 1.0 = 1%). Defaults to 1.0.
   * @returns The gasless claim result with transaction hash.
   *
   * @example
   * ```ts
   * const result = await client.claimViaGasless(swapId, "0xYourAddress");
   * console.log("Claimed! TX:", result.txHash);
   * ```
   */
  async claimViaGasless(
    id: string,
    destination: string,
    { slippage = 1.0 }: { slippage?: number } = {},
  ): Promise<ClaimGaslessResult> {
    if (!this.#swapStorage) {
      throw new Error(
        "Swap storage is not configured. Cannot retrieve preimage needed for gasless claim.",
      );
    }

    // Fetch all data upfront
    const stored = await this.#swapStorage.get(id);
    if (!stored) {
      throw new Error(`Swap ${id} not found in local storage.`);
    }

    const swap = (await this.getSwap(id, {
      updateStorage: true,
    })) as (ArkadeToEvmSwapResponse | LightningToEvmSwapResponse) & {
      direction: string;
    };

    if (
      swap.direction !== "arkade_to_evm" &&
      swap.direction !== "lightning_to_evm" &&
      swap.direction !== "bitcoin_to_evm"
    ) {
      throw new Error(
        `Expected arkade_to_evm or lightning_to_evm swap, got ${swap.direction}. claimViaGasless is for EVM-targeted swaps.`,
      );
    }

    // Always fetch redeem calldata from the server to get calls_hash.
    // For non-WBTC targets this also returns DEX calldata.
    const calldataResponse = await this.#apiClient.GET(
      "/swap/{id}/redeem-and-swap-calldata",
      {
        params: {
          path: { id },
          query: { destination, slippage },
        },
      },
    );
    if (calldataResponse.error) {
      throw new Error(
        `Failed to fetch redeem calldata: ${calldataResponse.error.error}`,
      );
    }

    // Cast to the updated response shape (includes calls_hash and optional dex_calldata).
    // The generated API types may lag behind the server; this will align after regeneration.
    const responseData = calldataResponse.data as {
      dex_calldata?: { to: string; data: string; value: string };
      gasless_fee_sats: number;
      calls_hash: string;
    };

    const targetTokenAddress = String(swap.target_token.token_id);
    const needsDexSwap =
      targetTokenAddress.toLowerCase() !== swap.wbtc_address.toLowerCase();

    let dexCalldata: { to: string; data: string; value: string } | undefined;
    if (needsDexSwap && responseData?.dex_calldata) {
      dexCalldata = {
        to: responseData.dex_calldata.to,
        data: responseData.dex_calldata.data,
        value: responseData.dex_calldata.value,
      };
    }

    const callsHash = responseData.calls_hash;

    return gaslessClaim({
      baseUrl: this.#config.baseUrl,
      preimage: stored.preimage,
      secretKey: hexToBytes(this.#getEvmSigningKey()),
      swap,
      destination,
      dexCalldata,
      callsHash,
    });
  }

  /**
   * Claims an Arkade (off-chain) VHTLC swap by revealing the preimage.
   *
   * Automatically selects the best claim method based on VTXO status:
   * - **spendable** VTXOs → offchain spend (submitTx/finalizeTx)
   * - **recoverable** or **mixed** VTXOs → delegated settlement via backend
   *
   * This is used for EVM-to-Arkade and BTC-to-Arkade swaps where the user
   * claims BTC on Arkade after the server has funded the VHTLC.
   *
   * @param id - The UUID of the swap.
   * @param options - Claim options including destination address.
   * @returns The claim result with transaction ID and amount.
   *
   * @example
   * ```ts
   * const result = await client.claimArkade(swapId, {
   *   destinationAddress: "ark1q...", // Where to receive BTC
   * });
   * if (result.success) {
   *   console.log("Claim TX:", result.txId);
   *   console.log("Amount:", result.claimAmount);
   * }
   * ```
   */
  async claimArkade(
    id: string,
    options: ArkadeClaimOptions,
  ): Promise<{
    success: boolean;
    message: string;
    txId?: string;
    claimAmount?: bigint;
  }> {
    // Validate options
    if (!options?.destinationAddress) {
      return {
        success: false,
        message:
          "Destination address is required for Arkade claims. " +
          'Provide it via the options parameter: { destinationAddress: "ark1..." }',
      };
    }

    // Check swap storage is configured
    if (!this.#swapStorage) {
      return {
        success: false,
        message:
          "Swap storage is not configured. Cannot retrieve the preimage needed for claim.",
      };
    }

    // Get stored swap data (contains preimage and secret key)
    const storedSwap = await this.#swapStorage.get(id);
    if (!storedSwap) {
      return {
        success: false,
        message: `Swap ${id} not found in local storage. The preimage is required to claim.`,
      };
    }

    const swap = storedSwap.response;

    // Ensure we have an Arkade-target swap
    if (
      swap.direction !== "btc_to_arkade" &&
      swap.direction !== "evm_to_arkade" &&
      swap.direction !== "lightning_to_arkade"
    ) {
      return {
        success: false,
        message: `Expected btc_to_arkade, lightning_to_arkade or evm_to_arkade swap, got ${swap.direction}. claimArkade is for swaps targeting Arkade.`,
      };
    }

    // Extract common VHTLC parameters
    const claimParams = this.#extractArkadeClaimParams(id, storedSwap);

    // Query VTXO status to determine claim method, polling briefly while
    // the Arkade indexer catches up if the funding tx hasn't been seen yet.
    const vtxoStatus = await this.#waitForVtxoStatus(
      id,
      options.waitForVtxoMs ?? 30_000,
    );

    if (vtxoStatus === "not_funded" || vtxoStatus === "spent") {
      return {
        success: false,
        message:
          vtxoStatus === "not_funded"
            ? `No VTXOs found at the VHTLC address ${claimParams.vhtlcAddress}. The swap may not have been funded yet.`
            : "All VTXOs have already been spent.",
      };
    }

    // Route based on VTXO status:
    // - spendable: offchain spend (faster, no backend dependency)
    // - recoverable/mixed: delegated settlement (handles expired batches)
    if (vtxoStatus === "spendable") {
      return this.#claimArkadeOffchain(claimParams, options);
    }

    // recoverable or mixed → delegate
    return this.#claimArkadeDelegate(id, claimParams, options);
  }

  /**
   * Poll `amountsForSwap` until the VTXO status leaves `not_funded` or
   * the timeout elapses. Returns the most recent status observed.
   *
   * Uses an exponential-ish backoff capped at 2s between probes so the
   * usual case (indexer catches up within a few seconds) feels snappy
   * without hammering the server.
   */
  async #waitForVtxoStatus(
    id: string,
    timeoutMs: number,
  ): Promise<VhtlcAmounts["vtxoStatus"]> {
    const deadline = Date.now() + Math.max(0, timeoutMs);
    let delayMs = 500;
    // First probe is immediate; subsequent probes back off.
    while (true) {
      const amounts = await this.amountsForSwap(id);
      if (amounts.vtxoStatus !== "not_funded") return amounts.vtxoStatus;
      if (Date.now() >= deadline) return amounts.vtxoStatus;
      const remaining = deadline - Date.now();
      const sleepMs = Math.min(delayMs, remaining);
      if (sleepMs <= 0) return amounts.vtxoStatus;
      await new Promise((resolve) => setTimeout(resolve, sleepMs));
      delayMs = Math.min(delayMs * 2, 2_000);
    }
  }

  /**
   * Continue (finalize) a pending Arkade claim.
   *
   * For Arkade-destination swaps (EVM/Bitcoin/Lightning → Arkade), this
   * fetches pending transactions from the Arkade server and finalizes them.
   * Use this when `claimArkade` submitted a claim but it wasn't finalized
   * (e.g. due to a page reload or network interruption).
   *
   * @param id - The UUID of the swap
   * @returns Result with txId and claimAmount on success
   */
  async continueArkadeClaimSwap(id: string): Promise<{
    success: boolean;
    message: string;
    txId?: string;
    claimAmount?: bigint;
  }> {
    if (!this.#swapStorage) {
      return {
        success: false,
        message: "Swap storage is not configured.",
      };
    }

    const storedSwap = await this.#swapStorage.get(id);
    if (!storedSwap) {
      return {
        success: false,
        message: `Swap ${id} not found in local storage.`,
      };
    }

    const swap = storedSwap.response;
    if (
      swap.direction !== "btc_to_arkade" &&
      swap.direction !== "evm_to_arkade" &&
      swap.direction !== "lightning_to_arkade"
    ) {
      return {
        success: false,
        message: `Expected an Arkade-destination swap, got ${swap.direction}.`,
      };
    }

    const claimParams = this.#extractArkadeClaimParams(id, storedSwap);

    try {
      const result = await continueArkadeClaim({
        ...claimParams,
        destinationAddress: "", // not needed for continue
        arkadeServerUrl: this.#config.arkadeServerUrl,
      });
      return {
        success: true,
        message: `Arkade claim finalized: ${result.txId}`,
        txId: result.txId,
        claimAmount: result.claimAmount,
      };
    } catch (err) {
      return {
        success: false,
        message: err instanceof Error ? err.message : String(err),
      };
    }
  }

  /**
   * Extracts VHTLC claim parameters from a stored swap.
   * @internal
   */
  #extractArkadeClaimParams(
    _id: string,
    storedSwap: StoredSwap,
  ): ArkadeVhtlcParams {
    const swap = storedSwap.response;
    const fullPubKey = storedSwap.publicKey;
    const userPubKey =
      fullPubKey.length === 66 ? fullPubKey.slice(2) : fullPubKey;

    let lendaswapPubKey: string;
    let arkadeServerPubKey: string;
    let vhtlcAddress: string;
    let refundLocktime: number;
    let unilateralClaimDelay: number;
    let unilateralRefundDelay: number;
    let unilateralRefundWithoutReceiverDelay: number;
    let network: string;

    if (swap.direction === "btc_to_arkade") {
      const s = swap as BtcToArkadeSwapResponse & {
        direction: "btc_to_arkade";
      };
      lendaswapPubKey = s.server_vhtlc_pk;
      arkadeServerPubKey = s.arkade_server_pk;
      vhtlcAddress = s.arkade_vhtlc_address;
      refundLocktime = s.vhtlc_refund_locktime;
      unilateralClaimDelay = s.unilateral_claim_delay;
      unilateralRefundDelay = s.unilateral_refund_delay;
      unilateralRefundWithoutReceiverDelay =
        s.unilateral_refund_without_receiver_delay;
      network = s.network;
    } else if (swap.direction === "evm_to_arkade") {
      const s = swap as {
        sender_pk: string;
        arkade_server_pk: string;
        btc_vhtlc_address: string;
        vhtlc_refund_locktime: number;
        unilateral_claim_delay: number;
        unilateral_refund_delay: number;
        unilateral_refund_without_receiver_delay: number;
        network: string;
      };
      lendaswapPubKey = s.sender_pk;
      arkadeServerPubKey = s.arkade_server_pk;
      vhtlcAddress = s.btc_vhtlc_address;
      refundLocktime = s.vhtlc_refund_locktime;
      unilateralClaimDelay = s.unilateral_claim_delay;
      unilateralRefundDelay = s.unilateral_refund_delay;
      unilateralRefundWithoutReceiverDelay =
        s.unilateral_refund_without_receiver_delay;
      network = s.network;
    } else if (swap.direction === "lightning_to_arkade") {
      lendaswapPubKey = swap.sender_pk;
      arkadeServerPubKey = swap.arkade_server_pk;
      vhtlcAddress = swap.arkade_vhtlc_address;
      refundLocktime = swap.vhtlc_refund_locktime;
      unilateralClaimDelay = swap.unilateral_claim_delay;
      unilateralRefundDelay = swap.unilateral_refund_delay;
      unilateralRefundWithoutReceiverDelay =
        swap.unilateral_refund_without_receiver_delay;
      network = swap.network;
    } else {
      throw Error(`Unsupported direction for Arkade claim: ${swap.direction}`);
    }

    return {
      userSecretKey: storedSwap.secretKey,
      userPubKey,
      lendaswapPubKey,
      arkadeServerPubKey,
      vhtlcAddress,
      refundLocktime,
      unilateralClaimDelay,
      unilateralRefundDelay,
      unilateralRefundWithoutReceiverDelay,
      network,
      preimage: storedSwap.preimage,
      preimageHash: storedSwap.preimageHash,
    };
  }

  /**
   * Claims via the offchain submitTx/finalizeTx path (spendable VTXOs only).
   * @internal
   */
  async #claimArkadeOffchain(
    params: ArkadeVhtlcParams,
    options: ArkadeClaimOptions,
  ): Promise<{
    success: boolean;
    message: string;
    txId?: string;
    claimAmount?: bigint;
  }> {
    try {
      const result = await buildArkadeClaim({
        ...params,
        destinationAddress: options.destinationAddress,
        arkadeServerUrl:
          options.arkadeServerUrl ?? this.#config.arkadeServerUrl,
      });

      return {
        success: true,
        message: "Arkade claim executed successfully via offchain spend!",
        txId: result.txId,
        claimAmount: result.claimAmount,
      };
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return {
        success: false,
        message: `Failed to execute offchain Arkade claim: ${message}`,
      };
    }
  }

  /**
   * Claims via the delegated settlement path (works for all VTXO states).
   * @internal
   */
  async #claimArkadeDelegate(
    swapId: string,
    params: ArkadeVhtlcParams,
    options: ArkadeClaimOptions,
  ): Promise<{
    success: boolean;
    message: string;
    txId?: string;
    claimAmount?: bigint;
  }> {
    try {
      const result = await delegateClaim({
        ...params,
        destinationAddress: options.destinationAddress,
        lendaswapApiUrl: this.#config.baseUrl,
        arkadeServerUrl:
          options.arkadeServerUrl ?? this.#config.arkadeServerUrl,
        swapId,
      });

      return {
        success: true,
        message: "Arkade claim executed successfully via delegated settlement!",
        txId: result.commitmentTxid,
      };
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return {
        success: false,
        message: `Failed to execute delegated Arkade claim: ${message}`,
      };
    }
  }

  // =========================================================================
  // Refund
  // =========================================================================

  /**
   * Attempts to refund a swap.
   *
   * Refund behavior depends on the swap type:
   * - **Lightning to EVM**: Cannot refund - Lightning swaps auto-expire if not completed.
   *   The invoice will simply expire and no funds are locked.
   * - **Arkade to EVM**: Off-chain refund via Arkade server
   * - **Bitcoin (on-chain) to EVM**: Builds a signed refund transaction that the user
   *   must broadcast to reclaim their funds after the locktime.
   *
   * @param id - The UUID of the swap to refund.
   * @param options - Options for on-chain refunds (required for btc_onchain swaps).
   * @returns A RefundResult with the transaction details (for on-chain) or status message.
   * @throws Error if the swap cannot be found, storage is not configured, or params are invalid.
   *
   * @example
   * ```ts
   * // For on-chain swaps
   * const result = await client.refundSwap(swapId, {
   *   destinationAddress: "bc1q...",
   *   feeRateSatPerVb: 5,
   * });
   * if (result.success) {
   *   console.log("Broadcast this transaction:", result.txHex);
   *   console.log("Transaction ID:", result.txId);
   * }
   * ```
   */
  async refundSwap(id: string, options?: RefundOptions): Promise<RefundResult> {
    // Get the swap to determine its type
    const storedSwap = await this.getStoredSwap(id);
    if (!storedSwap) {
      throw Error("Swap not found");
    }
    const swap = storedSwap.response;

    // Use direction to determine refund method (source_token may be a TokenSummary object)
    const direction = swap.direction;

    // Arkade swaps require off-chain refund
    if (direction === "arkade_to_evm") {
      return this.#buildArkadeRefund(id, swap, options as ArkadeRefundOptions);
    }

    // Bitcoin on-chain swaps require on-chain refund transaction
    if (direction === "bitcoin_to_evm" || direction === "btc_to_arkade") {
      return this.#buildOnchainRefund(
        id,
        swap,
        options as OnchainRefundOptions | undefined,
      );
    }

    // EVM-sourced swaps: collaborative refund or timelock-based refund
    if (
      direction === "evm_to_arkade" ||
      direction === "evm_to_bitcoin" ||
      direction === "evm_to_lightning"
    ) {
      const evmOptions = options as EvmRefundOptions | undefined;
      const settlement = evmOptions?.mode ?? "swap-back";

      if (evmOptions?.collaborative) {
        return this.#collabRefundEvm(id, settlement);
      }

      if (direction === "evm_to_arkade") {
        return this.#buildEvmToArkadeRefund(id, swap, settlement);
      }
      if (direction === "evm_to_bitcoin") {
        return this.#buildEvmToBitcoinRefund(id, swap, settlement);
      }
      return this.#buildEvmToLightningRefund(id, swap, settlement);
    }

    // Arkade-to-Lightning: collaborative refund or locktime refund
    if (direction === "arkade_to_lightning") {
      return this.#buildArkadeToLightningRefund(
        id,
        swap,
        options as ArkadeRefundOptions,
      );
    }

    return {
      success: false,
      message: `Refund not supported for direction: ${direction}.`,
    };
  }

  /**
   * Claims BTC from an on-chain Taproot HTLC for an EVM-to-Bitcoin swap.
   *
   * The user reveals the preimage to spend from the hashlock script path.
   * @internal
   */
  async #claimOnchainBtc(
    id: string,
    options?: ClaimOptions,
  ): Promise<ClaimResult> {
    if (!this.#swapStorage) {
      return {
        success: false,
        message:
          "Swap storage is not configured. Cannot retrieve preimage and keys needed for claim.",
      };
    }

    const storedSwap = await this.#swapStorage.get(id);
    if (!storedSwap) {
      return {
        success: false,
        message: `Swap ${id} not found in local storage.`,
      };
    }

    // Fetch the latest swap state from API
    const swap = (await this.getSwap(id, {
      updateStorage: false,
    })) as EvmToBitcoinSwapResponse & { direction: "evm_to_bitcoin" };

    if (swap.direction !== "evm_to_bitcoin") {
      return {
        success: false,
        message: `Expected evm_to_bitcoin swap, got ${swap.direction}`,
      };
    }

    // Extract BTC HTLC parameters
    const btcHtlcAddress = swap.btc_htlc_address;
    const btcHashLock = swap.btc_hash_lock;
    const btcRefundLocktime = swap.btc_refund_locktime;
    const networkStr = swap.network;

    // Get server refund pk (needed to reconstruct the Taproot tree)
    const serverRefundPkRaw = (swap as { btc_server_refund_pk?: string })
      .btc_server_refund_pk;
    if (!serverRefundPkRaw) {
      return {
        success: false,
        message:
          "Server refund public key not available. The API response may need to be updated.",
      };
    }

    // Map network string
    const networkMap: Record<string, BitcoinNetwork> = {
      bitcoin: "mainnet",
      mainnet: "mainnet",
      testnet: "testnet",
      signet: "signet",
      regtest: "regtest",
    };
    const network = networkMap[networkStr];
    if (!network) {
      return {
        success: false,
        message: `Unknown Bitcoin network: ${networkStr}`,
      };
    }

    // Get user's x-only public key (32 bytes from 33-byte compressed)
    const fullPubKey = storedSwap.publicKey;
    const userClaimPk =
      fullPubKey.length === 66 ? fullPubKey.slice(2) : fullPubKey;

    // Strip compressed key prefix if present
    const serverRefundPk =
      serverRefundPkRaw.length === 66
        ? serverRefundPkRaw.slice(2)
        : serverRefundPkRaw;

    // Verify HTLC address matches our reconstruction
    const addressMatches = verifyHtlcAddress(
      btcHtlcAddress,
      btcHashLock,
      userClaimPk, // claimer = user (goes in hashlock position)
      serverRefundPk, // refunder = server (goes in timelock position)
      btcRefundLocktime,
      network,
    );

    if (!addressMatches) {
      return {
        success: false,
        message:
          `HTLC address mismatch. Computed address does not match server's (${btcHtlcAddress}). ` +
          `Parameters: hashLock='${btcHashLock}', userPk='${userClaimPk}', ` +
          `serverPk='${serverRefundPk}', locktime='${btcRefundLocktime}', network='${network}'`,
      };
    }

    // Get the HTLC output info - prefer API data over Esplora lookup
    const esploraUrl = this.#config.esploraUrl ?? DEFAULT_ESPLORA_URLS[network];
    if (!esploraUrl) {
      return {
        success: false,
        message: `No Esplora URL configured for network ${network}.`,
      };
    }

    // Try to use funding info from the API response (faster, works before confirmation)
    const btcFundTxid = (swap as { btc_fund_txid?: string }).btc_fund_txid;
    const btcFundVout = (swap as { btc_fund_vout?: number }).btc_fund_vout;

    let htlcOutput: { txid: string; vout: number; amount: bigint } | null =
      null;

    if (btcFundTxid && btcFundVout !== undefined) {
      // We have the funding info from the API, but we need to get the amount
      // Query the transaction to get the output amount
      try {
        const txResponse = await fetch(`${esploraUrl}/tx/${btcFundTxid}`);
        if (txResponse.ok) {
          const txData = (await txResponse.json()) as {
            vout: Array<{ value: number }>;
          };
          if (txData.vout?.[btcFundVout]) {
            htlcOutput = {
              txid: btcFundTxid,
              vout: btcFundVout,
              amount: BigInt(txData.vout[btcFundVout].value),
            };
          }
        }
      } catch {
        // Fall through to Esplora lookup
      }
    }

    // Fallback: query Esplora for UTXOs at the address (requires confirmation)
    if (!htlcOutput) {
      htlcOutput = await findOutputByAddress(esploraUrl, btcHtlcAddress);
    }

    if (!htlcOutput) {
      return {
        success: false,
        message: `Could not find UTXO at HTLC address ${btcHtlcAddress}. The server may not have funded the HTLC yet.`,
      };
    }

    // Determine destination address: prefer explicit option, fall back to stored response
    const destinationAddress =
      options?.destinationAddress ??
      (swap as { target_btc_address?: string }).target_btc_address;
    if (!destinationAddress) {
      return {
        success: false,
        message:
          "Destination address is required to claim BTC. " +
          'Provide it via options: { destinationAddress: "bc1p..." }',
      };
    }

    try {
      const result = buildOnchainClaimTransaction({
        fundingTxId: htlcOutput.txid,
        fundingVout: htlcOutput.vout,
        htlcAmount: htlcOutput.amount,
        hashLock: btcHashLock,
        userClaimPubKey: userClaimPk,
        serverRefundPubKey: serverRefundPk,
        userSecretKey: storedSwap.secretKey,
        preimage: storedSwap.preimage,
        refundLocktime: btcRefundLocktime,
        destinationAddress,
        feeRateSatPerVb: options?.feeRateSatPerVb ?? 2,
        network,
      });

      // Broadcast
      try {
        await broadcastTransaction(esploraUrl, result.txHex);
        return {
          success: true,
          message: "BTC claim transaction broadcast successfully!",
          txHash: result.txId,
          // chain: "bitcoin" — not in ClaimChain type
        };
      } catch (broadcastError) {
        const msg =
          broadcastError instanceof Error
            ? broadcastError.message
            : String(broadcastError);
        return {
          success: true,
          message: `Claim transaction built but broadcast failed: ${msg}. TxHex: ${result.txHex}`,
          txHash: result.txId,
          // chain: "bitcoin" — not in ClaimChain type
        };
      }
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      return {
        success: false,
        message: `Failed to build claim transaction: ${msg}`,
      };
    }
  }

  /**
   * Builds an on-chain Bitcoin refund transaction.
   * @internal
   */
  async #buildOnchainRefund(
    id: string,
    swap: GetSwapResponse,
    options?: OnchainRefundOptions,
  ): Promise<RefundResult> {
    // Validate options
    if (!options?.destinationAddress) {
      return {
        success: false,
        message:
          "Destination address is required for on-chain refunds. " +
          'Provide it via the options parameter: { destinationAddress: "bc1q..." }',
      };
    }

    // Check swap storage is configured
    if (!this.#swapStorage) {
      return {
        success: false,
        message:
          "Swap storage is not configured. Cannot retrieve the secret key needed for refund.",
      };
    }

    // Get stored swap data (contains secret key)
    const storedSwap = await this.#swapStorage.get(id);
    if (!storedSwap) {
      return {
        success: false,
        message: `Swap ${id} not found in local storage. The secret key is required to sign the refund transaction.`,
      };
    }

    // Ensure we have an on-chain funded swap
    if (
      swap.direction !== "bitcoin_to_evm" &&
      swap.direction !== "btc_to_arkade"
    ) {
      return {
        success: false,
        message: `Expected bitcoin_to_evm or btc_to_arkade swap, got ${swap.direction}`,
      };
    }

    // Extract on-chain HTLC fields based on direction
    // Both directions have the same on-chain HTLC but fields are named differently
    let btcHtlcAddress: string;
    let btcRefundLocktime: number;
    let hashLock: string;
    let serverPubKeyFull: string;
    let networkStr: string;

    if (swap.direction === "btc_to_arkade") {
      const arkadeSwap = swap as BtcToArkadeSwapResponse & {
        direction: "btc_to_arkade";
      };
      btcHtlcAddress = arkadeSwap.btc_htlc_address;
      btcRefundLocktime = arkadeSwap.btc_refund_locktime;
      hashLock = arkadeSwap.hash_lock;
      serverPubKeyFull = arkadeSwap.server_vhtlc_pk;
      networkStr = arkadeSwap.network;
    } else {
      // OnchainToEvmSwapResponse (on-chain Bitcoin to EVM)
      const onchainSwap = swap as unknown as {
        btc_htlc_address: string;
        btc_refund_locktime: number;
        btc_hash_lock: string;
        btc_server_pk: string;
        network: string;
      };
      btcHtlcAddress = onchainSwap.btc_htlc_address;
      btcRefundLocktime = onchainSwap.btc_refund_locktime;
      hashLock = onchainSwap.btc_hash_lock;
      serverPubKeyFull = onchainSwap.btc_server_pk;
      networkStr = onchainSwap.network;
    }

    // Check refund locktime
    const now = Math.floor(Date.now() / 1000);
    if (now < btcRefundLocktime) {
      const remainingSeconds = btcRefundLocktime - now;
      const remainingMinutes = Math.ceil(remainingSeconds / 60);
      return {
        success: false,
        message:
          `Refund is not yet available. The locktime expires in ${remainingMinutes} minutes ` +
          `(at ${new Date(btcRefundLocktime * 1000).toISOString()}).`,
      };
    }

    // Map network string to BitcoinNetwork type
    const networkMap: Record<string, BitcoinNetwork> = {
      bitcoin: "mainnet",
      mainnet: "mainnet",
      testnet: "testnet",
      signet: "signet",
      regtest: "regtest",
    };
    const network = networkMap[networkStr];
    if (!network) {
      return {
        success: false,
        message: `Unknown Bitcoin network: ${networkStr}`,
      };
    }

    // Get user's x-only public key (32 bytes) from stored swap
    // The stored publicKey is the full compressed pubkey (33 bytes)
    // We need to extract the x-only portion (drop the first byte prefix)
    const fullPubKey = storedSwap.publicKey;
    const userPubKey =
      fullPubKey.length === 66 ? fullPubKey.slice(2) : fullPubKey;

    // Strip compressed key prefix if present (33-byte → 32-byte x-only)
    const serverXOnlyPubKey =
      serverPubKeyFull.length === 66
        ? serverPubKeyFull.slice(2)
        : serverPubKeyFull;

    // Verify that our computed HTLC address matches the server's address
    const addressMatches = verifyHtlcAddress(
      btcHtlcAddress,
      hashLock,
      serverXOnlyPubKey,
      userPubKey,
      btcRefundLocktime,
      network,
    );

    if (!addressMatches) {
      return {
        success: false,
        message:
          `HTLC address mismatch. The computed address does not match the server's address (${btcHtlcAddress}). ` +
          `This could indicate different script construction. ` +
          `Parameters: \nhashLock='${hashLock}', \nserverPk='${serverPubKeyFull}', ` +
          `\nuserPk='${userPubKey}', \nlocktime='${btcRefundLocktime}',` +
          `\nnetwork='${network}'`,
      };
    }

    // Get the HTLC output info - prefer API data over Esplora lookup
    const esploraUrl = this.#config.esploraUrl ?? DEFAULT_ESPLORA_URLS[network];
    if (!esploraUrl) {
      return {
        success: false,
        message: `No Esplora URL configured for network ${network}. Cannot look up funding transaction.`,
      };
    }

    // Try to use funding info from the API response (faster, works before confirmation)
    const btcFundTxid = (swap as { btc_fund_txid?: string }).btc_fund_txid;
    const btcFundVout = (swap as { btc_fund_vout?: number }).btc_fund_vout;

    let htlcOutput: { txid: string; vout: number; amount: bigint } | null =
      null;

    if (btcFundTxid && btcFundVout !== undefined) {
      // We have the funding info from the API, get the amount from the transaction
      try {
        const txResponse = await fetch(`${esploraUrl}/tx/${btcFundTxid}`);
        if (txResponse.ok) {
          const txData = (await txResponse.json()) as {
            vout: Array<{ value: number }>;
          };
          if (txData.vout?.[btcFundVout]) {
            htlcOutput = {
              txid: btcFundTxid,
              vout: btcFundVout,
              amount: BigInt(txData.vout[btcFundVout].value),
            };
          }
        }
      } catch {
        // Fall through to Esplora lookup
      }
    }

    // Fallback: query Esplora for UTXOs at the address (requires confirmation)
    if (!htlcOutput) {
      htlcOutput = await findOutputByAddress(esploraUrl, btcHtlcAddress);
    }

    if (!htlcOutput) {
      return {
        success: false,
        message:
          `Could not find UTXO at HTLC address ${btcHtlcAddress}. ` +
          `The address may not have been funded yet.`,
      };
    }

    try {
      // Build the refund transaction
      const result = buildOnchainRefundTransaction({
        fundingTxId: htlcOutput.txid,
        fundingVout: htlcOutput.vout,
        htlcAmount: htlcOutput.amount,
        hashLock,
        serverPubKey: serverXOnlyPubKey,
        userPubKey,
        userSecretKey: storedSwap.secretKey,
        refundLocktime: btcRefundLocktime,
        destinationAddress: options.destinationAddress,
        feeRateSatPerVb: options.feeRateSatPerVb ?? 2,
        network,
      });

      // If dry run, just return the transaction without broadcasting
      if (options.dryRun) {
        return {
          success: true,
          message:
            "Refund transaction built successfully (dry run - not broadcast).",
          txHex: result.txHex,
          txId: result.txId,
          refundAmount: result.refundAmount,
          fee: result.fee,
          broadcast: false,
          htlcAddress: result.htlcAddress,
          serverHtlcAddress: btcHtlcAddress,
        };
      }

      // Broadcast the transaction
      const broadcastEsploraUrl =
        this.#config.esploraUrl ?? DEFAULT_ESPLORA_URLS[network];
      if (!broadcastEsploraUrl) {
        return {
          success: true,
          message:
            "Refund transaction built successfully. No Esplora URL configured for broadcast. " +
            "Broadcast the txHex manually to the Bitcoin network.",
          txHex: result.txHex,
          txId: result.txId,
          refundAmount: result.refundAmount,
          fee: result.fee,
          broadcast: false,
          htlcAddress: result.htlcAddress,
          serverHtlcAddress: btcHtlcAddress,
        };
      }

      try {
        await broadcastTransaction(broadcastEsploraUrl, result.txHex);
        return {
          success: true,
          message: "Refund transaction broadcast successfully!",
          txHex: result.txHex,
          txId: result.txId,
          refundAmount: result.refundAmount,
          fee: result.fee,
          broadcast: true,
          htlcAddress: result.htlcAddress,
          serverHtlcAddress: btcHtlcAddress,
        };
      } catch (broadcastError) {
        const broadcastMessage =
          broadcastError instanceof Error
            ? broadcastError.message
            : String(broadcastError);
        return {
          success: true,
          message:
            `Transaction built but broadcast failed: ${broadcastMessage}. ` +
            "You can broadcast the txHex manually.",
          txHex: result.txHex,
          txId: result.txId,
          refundAmount: result.refundAmount,
          fee: result.fee,
          broadcast: false,
          htlcAddress: result.htlcAddress,
          serverHtlcAddress: btcHtlcAddress,
        };
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return {
        success: false,
        message: `Failed to build refund transaction: ${message}`,
      };
    }
  }

  /**
   * Builds and executes an Arkade (off-chain) VHTLC refund.
   *
   * Automatically selects the best refund method based on VTXO status:
   * - **spendable** VTXOs → offchain spend (submitTx/finalizeTx)
   * - **recoverable** or **mixed** VTXOs → delegated settlement via backend
   *
   * @internal
   */
  async #buildArkadeRefund(
    id: string,
    swap: GetSwapResponse,
    options?: ArkadeRefundOptions,
  ): Promise<RefundResult> {
    // Validate options
    if (!options?.destinationAddress) {
      return {
        success: false,
        message:
          "Destination address is required for Arkade refunds. " +
          'Provide it via the options parameter: { destinationAddress: "ark1..." }',
      };
    }

    // Check swap storage is configured
    if (!this.#swapStorage) {
      return {
        success: false,
        message:
          "Swap storage is not configured. Cannot retrieve the secret key needed for refund.",
      };
    }

    // Get stored swap data (contains secret key)
    const storedSwap = await this.#swapStorage.get(id);
    if (!storedSwap) {
      return {
        success: false,
        message: `Swap ${id} not found in local storage. The secret key is required to sign the refund transaction.`,
      };
    }

    // Ensure we have an arkade_to_evm swap
    if (swap.direction !== "arkade_to_evm") {
      return {
        success: false,
        message: `Expected arkade_to_evm swap, got ${swap.direction}`,
      };
    }

    const s = swap as ArkadeToEvmSwapResponse & {
      direction: "arkade_to_evm";
    };

    const fullPubKey = storedSwap.publicKey;
    const userPubKey =
      fullPubKey.length === 66 ? fullPubKey.slice(2) : fullPubKey;

    const hashLock = s.hash_lock.startsWith("0x")
      ? s.hash_lock.slice(2)
      : s.hash_lock;

    // Query VTXO status to determine refund method
    const amounts = await this.amountsForSwap(id);
    const vtxoStatus = amounts.vtxoStatus;

    if (vtxoStatus === "not_funded" || vtxoStatus === "spent") {
      return {
        success: false,
        message:
          vtxoStatus === "not_funded"
            ? "No VTXOs found at the VHTLC address."
            : "All VTXOs have already been spent.",
      };
    }

    const refundParams = {
      userSecretKey: storedSwap.secretKey,
      userPubKey,
      lendaswapPubKey: s.receiver_pk,
      arkadeServerPubKey: s.arkade_server_pk,
      hashLock,
      vhtlcAddress: s.btc_vhtlc_address,
      refundLocktime: s.vhtlc_refund_locktime,
      unilateralClaimDelay: s.unilateral_claim_delay,
      unilateralRefundDelay: s.unilateral_refund_delay,
      unilateralRefundWithoutReceiverDelay:
        s.unilateral_refund_without_receiver_delay,
      destinationAddress: options.destinationAddress,
      network: s.network,
    };

    // Try collaborative refund first (instant, no locktime wait).
    // Falls back to non-collab refund if the server rejects (e.g. unsafe state).
    const collabParams = {
      ...refundParams,
      swapId: id,
      apiClient: this.#apiClient,
      arkadeServerUrl: options.arkadeServerUrl ?? this.#config.arkadeServerUrl,
    };

    try {
      if (vtxoStatus === "spendable") {
        const result = await collabRefundArkadeToEvmOffchain(collabParams);
        return {
          success: true,
          message:
            "Arkade refund executed successfully via collaborative offchain spend!",
          txId: result.txId,
          refundAmount: result.refundAmount,
          broadcast: true,
        };
      }
      // recoverable or mixed → collab delegate
      const result = await collabRefundArkadeToEvmDelegate(collabParams);
      return {
        success: true,
        message:
          "Arkade refund executed successfully via collaborative delegated settlement!",
        txId: result.commitmentTxid,
        broadcast: true,
      };
    } catch (collabError) {
      const collabMsg =
        collabError instanceof Error
          ? collabError.message
          : String(collabError);
      console.warn(
        `Collaborative refund failed (${collabMsg}), falling back to non-collab refund`,
      );
    }

    // Fallback: non-collaborative refund (requires locktime to have expired)
    const now = Math.floor(Date.now() / 1000);
    if (now < s.vhtlc_refund_locktime) {
      const remainingSeconds = s.vhtlc_refund_locktime - now;
      const remainingMinutes = Math.ceil(remainingSeconds / 60);
      return {
        success: false,
        message:
          `Collaborative refund was rejected by the server and non-collaborative refund ` +
          `is not yet available. The VHTLC locktime expires in ${remainingMinutes} minutes ` +
          `(at ${new Date(s.vhtlc_refund_locktime * 1000).toISOString()}).`,
      };
    }

    if (vtxoStatus === "spendable") {
      return this.#refundArkadeOffchain(refundParams, options);
    }

    // recoverable or mixed → delegate
    return this.#refundArkadeDelegate(refundParams, options);
  }

  /**
   * Refund an Arkade-to-Lightning swap.
   *
   * Two paths:
   * 1. **Collaborative refund** (instant) — available when status is `serverwontfund` or `clientinvalidfunded`
   * 2. **Locktime refund** (after CLTV expiry) — fallback when the receiver doesn't cooperate
   *
   * For retrying with a new invoice, use {@link retryArkadeToLightningSwap} instead.
   * @internal
   */
  async #buildArkadeToLightningRefund(
    id: string,
    swap: GetSwapResponse,
    options?: ArkadeRefundOptions,
  ): Promise<RefundResult> {
    if (!options?.destinationAddress) {
      return {
        success: false,
        message:
          "Destination address is required for Arkade refunds. " +
          'Provide it via the options parameter: { destinationAddress: "ark1..." }',
      };
    }

    if (!this.#swapStorage) {
      return {
        success: false,
        message:
          "Swap storage is not configured. Cannot retrieve the secret key needed for refund.",
      };
    }

    const storedSwap = await this.#swapStorage.get(id);
    if (!storedSwap) {
      return {
        success: false,
        message: `Swap ${id} not found in local storage. The secret key is required to sign the refund transaction.`,
      };
    }

    if (swap.direction !== "arkade_to_lightning") {
      return {
        success: false,
        message: `Expected arkade_to_lightning swap, got ${swap.direction}`,
      };
    }

    const s = swap as ArkadeToLightningSwapResponse & {
      direction: "arkade_to_lightning";
    };

    const fullPubKey = storedSwap.publicKey;
    const userPubKey =
      fullPubKey.length === 66 ? fullPubKey.slice(2) : fullPubKey;

    const hashLock = s.hash_lock.startsWith("0x")
      ? s.hash_lock.slice(2)
      : s.hash_lock;

    // Try collaborative refund first (instant)
    try {
      // TODO: Add collabRefundArkadeToLightningDelegate
      const result = await collabRefundArkadeToLightningOffchain({
        userSecretKey: storedSwap.secretKey,
        userPubKey,
        receiverPubKey: s.receiver_pk,
        arkadeServerPubKey: s.arkade_server_pk,
        hashLock,
        vhtlcAddress: s.arkade_vhtlc_address,
        refundLocktime: s.vhtlc_refund_locktime,
        unilateralClaimDelay: s.unilateral_claim_delay,
        unilateralRefundDelay: s.unilateral_refund_delay,
        unilateralRefundWithoutReceiverDelay:
          s.unilateral_refund_without_receiver_delay,
        destinationAddress: options.destinationAddress,
        network: s.network,
        arkadeServerUrl:
          options.arkadeServerUrl ?? this.#config.arkadeServerUrl,
        swapId: id,
        apiClient: this.#apiClient,
      });

      return {
        success: true,
        message:
          "Arkade-to-Lightning refund executed via collaborative refund!",
        txId: result.txId,
        refundAmount: result.refundAmount,
        broadcast: true,
      };
    } catch (collabError) {
      const collabMsg =
        collabError instanceof Error
          ? collabError.message
          : String(collabError);
      console.warn(
        `collaborative refund failed (${collabMsg}), checking locktime fallback`,
      );
    }

    // Fallback: non-collaborative refund (requires locktime to have expired)
    // TODO: Should use Bitcoin's MTP.
    const now = Math.floor(Date.now() / 1000);
    if (now < s.vhtlc_refund_locktime) {
      const remainingSeconds = s.vhtlc_refund_locktime - now;
      const remainingMinutes = Math.ceil(remainingSeconds / 60);
      return {
        success: false,
        message:
          `collaborative refund failed and non-collaborative refund ` +
          `is not yet available. The VHTLC locktime expires in ${remainingMinutes} minutes ` +
          `(at ${new Date(s.vhtlc_refund_locktime * 1000).toISOString()}). ` +
          `Try again after the locktime expires, or use retryArkadeToLightningSwap() to ` +
          `retry with a new Lightning invoice.`,
      };
    }

    // Locktime expired — use refund_without_receiver path (2-of-2: sender + server)
    // This reuses the existing Arkade refund logic — lendaswapPubKey is set to
    // the receiver key from this swap's VHTLC script.
    // TODO: Not yet e2e-tested!
    const refundParams = {
      userSecretKey: storedSwap.secretKey,
      userPubKey,
      lendaswapPubKey: s.receiver_pk,
      arkadeServerPubKey: s.arkade_server_pk,
      hashLock,
      vhtlcAddress: s.arkade_vhtlc_address,
      refundLocktime: s.vhtlc_refund_locktime,
      unilateralClaimDelay: s.unilateral_claim_delay,
      unilateralRefundDelay: s.unilateral_refund_delay,
      unilateralRefundWithoutReceiverDelay:
        s.unilateral_refund_without_receiver_delay,
      destinationAddress: options.destinationAddress,
      network: s.network,
    };

    // Query VTXO status to determine refund method
    const amounts = await this.amountsForSwap(id);
    const vtxoStatus = amounts.vtxoStatus;

    if (vtxoStatus === "not_funded" || vtxoStatus === "spent") {
      return {
        success: false,
        message:
          vtxoStatus === "not_funded"
            ? "No VTXOs found at the VHTLC address."
            : "All VTXOs have already been spent.",
      };
    }

    if (vtxoStatus === "spendable") {
      return this.#refundArkadeOffchain(refundParams, options);
    }

    return this.#refundArkadeDelegate(refundParams, options);
  }

  /**
   * Refunds via the offchain submitTx/finalizeTx path (spendable VTXOs only).
   * @internal
   */
  async #refundArkadeOffchain(
    params: {
      userSecretKey: string;
      userPubKey: string;
      lendaswapPubKey: string;
      arkadeServerPubKey: string;
      hashLock: string;
      vhtlcAddress: string;
      refundLocktime: number;
      unilateralClaimDelay: number;
      unilateralRefundDelay: number;
      unilateralRefundWithoutReceiverDelay: number;
      destinationAddress: string;
      network: string;
    },
    options: ArkadeRefundOptions,
  ): Promise<RefundResult> {
    try {
      const result = await buildArkadeRefund({
        ...params,
        arkadeServerUrl:
          options.arkadeServerUrl ?? this.#config.arkadeServerUrl,
      });

      return {
        success: true,
        message: "Arkade refund executed successfully via offchain spend!",
        txId: result.txId,
        refundAmount: result.refundAmount,
        broadcast: true,
      };
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return {
        success: false,
        message: `Failed to execute offchain Arkade refund: ${message}`,
      };
    }
  }

  /**
   * Refunds via the delegated settlement path (works for all VTXO states).
   * @internal
   */
  async #refundArkadeDelegate(
    params: {
      userSecretKey: string;
      userPubKey: string;
      lendaswapPubKey: string;
      arkadeServerPubKey: string;
      hashLock: string;
      vhtlcAddress: string;
      refundLocktime: number;
      unilateralClaimDelay: number;
      unilateralRefundDelay: number;
      unilateralRefundWithoutReceiverDelay: number;
      destinationAddress: string;
      network: string;
    },
    options: ArkadeRefundOptions,
  ): Promise<RefundResult> {
    try {
      const result = await delegateRefund({
        ...params,
        lendaswapApiUrl: this.#config.baseUrl,
        arkadeServerUrl:
          options.arkadeServerUrl ?? this.#config.arkadeServerUrl,
      });

      return {
        success: true,
        message:
          "Arkade refund executed successfully via delegated settlement!",
        txId: result.commitmentTxid,
        broadcast: true,
      };
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return {
        success: false,
        message: `Failed to execute delegated Arkade refund: ${message}`,
      };
    }
  }

  /**
   * Builds refund data for an EVM-to-Arkade swap via the coordinator.
   *
   * Calls the server's refund-calldata endpoint which builds coordinator
   * calldata for `refundAndExecute` (swap WBTC back to source token) or
   * `refundTo` (return WBTC directly).
   *
   * @internal
   */
  async #buildEvmToArkadeRefund(
    id: string,
    swap: GetSwapResponse,
    mode: "swap-back" | "direct" = "swap-back",
  ): Promise<RefundResult> {
    const evmSwap = swap as EvmToArkadeSwapResponse & {
      direction: "evm_to_arkade";
    };

    const timelock = evmSwap.evm_refund_locktime;
    const now = Math.floor(Date.now() / 1000);
    const timelockExpired = now >= timelock;

    // Check if source token is BTC-pegged (WBTC/tBTC) - if so, use direct HTLCErc20 refund
    const isWbtcSource = evmSwap.source_token
      ? isBtcPegged(evmSwap.source_token)
      : false;

    if (isWbtcSource) {
      // Direct HTLCErc20 refund - no DEX swap needed
      const htlcAddress = evmSwap.evm_htlc_address;
      const hashLock = evmSwap.hash_lock;

      const refundData = encodeHtlcErc20RefundCallData(htlcAddress, {
        preimageHash: hashLock,
        amount: BigInt(evmSwap.source_amount),
        token: evmSwap.source_token.token_id,
        claimAddress: evmSwap.server_evm_address, // The server would have been the claimer
        timelock: timelock,
      });

      return {
        success: true,
        message: timelockExpired
          ? "EVM refund calldata ready. Submit this transaction with your EVM wallet."
          : `Timelock has not expired yet. Refund will be available at ${new Date(timelock * 1000).toISOString()}.`,
        evmRefundData: {
          to: refundData.to,
          data: refundData.data,
          timelockExpired,
          timelockExpiry: timelock,
        },
      };
    }

    // Non-WBTC source: fetch coordinator refund calldata from server
    // - "swap-back": swap WBTC back to original token via DEX (default)
    // - "direct": return WBTC directly (useful when DEX calldata is stale)
    const response = await this.#apiClient.GET(
      "/swap/{id}/refund-and-swap-calldata",
      {
        params: {
          path: { id },
          query: { mode },
        },
      },
    );

    if (response.error) {
      return {
        success: false,
        message: `Failed to fetch refund calldata: ${response.error.error || "Unknown error"}`,
      };
    }

    const { coordinator_address, calldata } = response.data;

    return {
      success: true,
      message: timelockExpired
        ? "EVM refund calldata ready. Submit this transaction with your EVM wallet."
        : `Timelock has not expired yet. Refund will be available at ${new Date(timelock * 1000).toISOString()}.`,
      evmRefundData: {
        to: coordinator_address,
        data: calldata,
        timelockExpired,
        timelockExpiry: timelock,
      },
    };
  }

  /**
   * Builds refund data for an EVM-to-Bitcoin swap via the coordinator.
   * Same pattern as EVM-to-Arkade: uses the coordinator refund-and-swap-calldata endpoint.
   * @internal
   */
  async #buildEvmToBitcoinRefund(
    id: string,
    swap: GetSwapResponse,
    mode: "swap-back" | "direct" = "swap-back",
  ): Promise<RefundResult> {
    const evmSwap = swap as EvmToBitcoinSwapResponse & {
      direction: "evm_to_bitcoin";
    };

    const timelock = evmSwap.evm_refund_locktime;
    const now = Math.floor(Date.now() / 1000);
    const timelockExpired = now >= timelock;

    // Check if source token is BTC-pegged (WBTC/tBTC) - if so, use direct HTLCErc20 refund
    const isWbtcSource = evmSwap.source_token
      ? isBtcPegged(evmSwap.source_token)
      : false;

    if (isWbtcSource) {
      // Direct HTLCErc20 refund - no DEX swap needed
      const htlcAddress = evmSwap.evm_htlc_address;
      const hashLock = evmSwap.evm_hash_lock;

      const refundData = encodeHtlcErc20RefundCallData(htlcAddress, {
        preimageHash: hashLock,
        amount: BigInt(evmSwap.source_amount),
        token: evmSwap.source_token.token_id,
        claimAddress: evmSwap.server_evm_address, // The server would have been the claimer
        timelock: timelock,
      });

      return {
        success: true,
        message: timelockExpired
          ? "EVM refund calldata ready. Submit this transaction with your EVM wallet."
          : `Timelock has not expired yet. Refund will be available at ${new Date(timelock * 1000).toISOString()}.`,
        evmRefundData: {
          to: refundData.to,
          data: refundData.data,
          timelockExpired,
          timelockExpiry: timelock,
        },
      };
    }

    // Non-WBTC source: use coordinator refund
    // - "swap-back": swap WBTC back to original token via DEX (default)
    // - "direct": return WBTC directly (useful when DEX calldata is stale)
    const response = await this.#apiClient.GET(
      "/swap/{id}/refund-and-swap-calldata",
      {
        params: {
          path: { id },
          query: { mode },
        },
      },
    );

    if (response.error) {
      return {
        success: false,
        message: `Failed to fetch refund calldata: ${response.error.error || "Unknown error"}`,
      };
    }

    const { coordinator_address, calldata } = response.data;

    return {
      success: true,
      message: timelockExpired
        ? "EVM refund calldata ready. Submit this transaction with your EVM wallet."
        : `Timelock has not expired yet. Refund will be available at ${new Date(timelock * 1000).toISOString()}.`,
      evmRefundData: {
        to: coordinator_address,
        data: calldata,
        timelockExpired,
        timelockExpiry: timelock,
      },
    };
  }

  /**
   * Builds refund data for an EVM-to-Lightning swap via the coordinator.
   *
   * Like EVM-to-Arkade, the coordinator atomically swapped the source token to WBTC
   * before locking in the HTLC. For refunds:
   * - If source was WBTC: direct HTLCErc20 refund
   * - Otherwise: use coordinator refund endpoint (swap-back or direct mode)
   *
   * @internal
   */
  async #buildEvmToLightningRefund(
    id: string,
    swap: GetSwapResponse,
    mode: "swap-back" | "direct" = "swap-back",
  ): Promise<RefundResult> {
    const evmSwap = swap as EvmToLightningSwapResponse & {
      direction: "evm_to_lightning";
    };

    const timelock = evmSwap.evm_refund_locktime;
    const now = Math.floor(Date.now() / 1000);
    const timelockExpired = now >= timelock;

    // Check if source token is BTC-pegged (WBTC/tBTC) - if so, use direct HTLCErc20 refund
    const isWbtcSource = evmSwap.source_token
      ? isBtcPegged(evmSwap.source_token)
      : false;

    if (isWbtcSource) {
      // Direct HTLCErc20 refund - no DEX swap needed
      const htlcAddress = evmSwap.evm_htlc_address;
      const hashLock = evmSwap.hash_lock;

      const refundData = encodeHtlcErc20RefundCallData(htlcAddress, {
        preimageHash: hashLock,
        amount: BigInt(evmSwap.source_amount),
        token: evmSwap.source_token.token_id,
        claimAddress: evmSwap.server_evm_address,
        timelock: timelock,
      });

      return {
        success: true,
        message: timelockExpired
          ? "EVM refund calldata ready. Submit this transaction with your EVM wallet."
          : `Timelock has not expired yet. Refund will be available at ${new Date(timelock * 1000).toISOString()}.`,
        evmRefundData: {
          to: refundData.to,
          data: refundData.data,
          timelockExpired,
          timelockExpiry: timelock,
        },
      };
    }

    // Non-WBTC source: fetch coordinator refund calldata from server
    // - "swap-back": swap WBTC back to original token via DEX (default)
    // - "direct": return WBTC directly (useful when DEX calldata is stale)
    const response = await this.#apiClient.GET(
      "/swap/{id}/refund-and-swap-calldata",
      {
        params: {
          path: { id },
          query: { mode },
        },
      },
    );

    if (response.error) {
      return {
        success: false,
        message: `Failed to fetch refund calldata: ${response.error.error || "Unknown error"}`,
      };
    }

    const { coordinator_address, calldata } = response.data;

    return {
      success: true,
      message: timelockExpired
        ? "EVM refund calldata ready. Submit this transaction with your EVM wallet."
        : `Timelock has not expired yet. Refund will be available at ${new Date(timelock * 1000).toISOString()}.`,
      evmRefundData: {
        to: coordinator_address,
        data: calldata,
        timelockExpired,
        timelockExpiry: timelock,
      },
    };
  }

  // =========================================================================
  // Collaborative EVM Refund
  // =========================================================================

  /**
   * Fetches the EIP-712 parameters for collaborative EVM HTLC refund.
   *
   * Returns the addresses and values needed to build the `CollabRefund`
   * EIP-712 typed data that the depositor signs.
   *
   * @param swapId - Swap ID
   * @returns CollabRefund parameters
   */
  async getCollabRefundEvmParams(
    swapId: string,
    settlement: "swap-back" | "direct" = "direct",
  ): Promise<CollabRefundEvmParams> {
    const response = await this.#apiClient.GET(
      "/api/swap/{id}/collab-refund-evm/params",
      {
        params: {
          path: { id: swapId },
          query: { mode: settlement },
        },
      },
    );

    if (response.error) {
      throw new Error(
        `Failed to fetch collab refund params: ${response.error.error || "Unknown error"}`,
      );
    }

    const d = response.data;
    return {
      coordinatorAddress: d.coordinator_address,
      serverSignerAddress: d.server_signer_address,
      preimageHash: d.preimage_hash,
      amount: d.amount,
      token: d.token,
      claimAddress: d.claim_address,
      timelock: d.timelock,
      chainId: d.chain_id,
      mode: d.mode,
      sweepToken: d.sweep_token,
      minAmountOut: d.min_amount_out,
      callsHash: d.calls_hash,
      sourceTokenAddress: d.source_token_address ?? undefined,
      dexCalldata: d.dex_calldata
        ? {
            to: d.dex_calldata.to,
            data: d.dex_calldata.data,
            value: d.dex_calldata.value,
          }
        : undefined,
    };
  }

  /**
   * Builds the EIP-712 typed data for collaborative EVM refund.
   *
   * The depositor signs this with their wallet (via `eth_signTypedData_v4`
   * or the SDK's `signEvmDigest`) to authorize the server to submit the
   * refund on-chain.
   *
   * @param swapId - Swap ID
   * @param settlement - Settlement mode: "direct" (WBTC) or "swap-back" (original token via DEX)
   * @returns Typed data and digest for signing
   */
  async buildCollabRefundEvmTypedData(
    swapId: string,
    settlement: "swap-back" | "direct" = "direct",
  ): Promise<{
    typedData: CollabRefundEvmTypedData;
    digest: string;
    params: CollabRefundEvmParams;
  }> {
    const params = await this.getCollabRefundEvmParams(swapId, settlement);

    const digestParams: CollabRefundEvmDigestParams = {
      coordinatorAddress: params.coordinatorAddress,
      chainId: params.chainId,
      preimageHash: params.preimageHash,
      amount: BigInt(params.amount),
      token: params.token,
      claimAddress: params.claimAddress,
      timelock: params.timelock,
      caller: params.serverSignerAddress,
      sweepToken: params.sweepToken,
      minAmountOut: BigInt(params.minAmountOut),
      callsHash: params.callsHash,
    };

    const typedData = buildCollabRefundEvmTypedData(digestParams);
    const digest = buildCollabRefundEvmDigest(digestParams);

    return { typedData, digest, params };
  }

  /**
   * Performs a collaborative EVM refund using the SDK's embedded wallet.
   *
   * The SDK signs the EIP-712 `CollabRefund` digest with the depositor's
   * derived EVM key and POSTs to the server, which cosigns and submits
   * the transaction on-chain. Gasless for the client — no timelock wait.
   *
   * @param swapId - Swap ID
   * @param settlement - Settlement mode: "direct" (WBTC) or "swap-back" (original token via DEX)
   * @returns Refund result with transaction hash
   */
  async collabRefundEvmSwap(
    swapId: string,
    settlement: "swap-back" | "direct" = "direct",
  ): Promise<CollabRefundEvmResult> {
    const { params, digest } = await this.buildCollabRefundEvmTypedData(
      swapId,
      settlement,
    );

    // Sign using the EVM secret key for this swap
    const storedSwap = await this.getStoredSwap(swapId);
    if (!storedSwap?.secretKey) {
      throw new Error(
        "No secret key found for this swap. Cannot sign collab refund.",
      );
    }
    const evmKey = this.#getEvmSigningKey();
    const sig = signEvmDigest(evmKey, digest);

    // Derive the on-chain depositor address from the EVM signing key
    const depositorAddress = deriveEvmAddress(evmKey);

    // POST to the server
    const response = await this.#apiClient.POST(
      "/api/swap/{id}/collab-refund-evm",
      {
        params: { path: { id: swapId } },
        body: {
          v: sig.v,
          r: sig.r,
          s: sig.s,
          depositor_address: depositorAddress,
          mode: settlement,
          sweep_token: params.sweepToken,
          min_amount_out: params.minAmountOut,
        },
      },
    );

    if (response.error) {
      throw new Error(
        `Collaborative EVM refund failed: ${response.error.error || "Unknown error"}`,
      );
    }

    return {
      id: response.data.id,
      txHash: response.data.tx_hash,
      message: response.data.message,
    };
  }

  /**
   * Submits a pre-signed collaborative EVM refund.
   *
   * Use this when an external wallet (e.g. MetaMask) signs the EIP-712
   * `CollabRefund` digest instead of the SDK's embedded key.
   * Call {@link buildCollabRefundEvmTypedData} first to obtain the typed data
   * for the wallet to sign, then pass the resulting signature here.
   *
   * @param swapId - Swap ID
   * @param body   - Signed refund request (signature + refund parameters)
   * @returns Refund result with transaction hash
   */
  async submitCollabRefundEvm(
    swapId: string,
    body: {
      v: number;
      r: string;
      s: string;
      depositor_address: string;
      mode: "direct" | "swap-back";
      sweep_token?: string;
      min_amount_out: string;
    },
  ): Promise<CollabRefundEvmResult> {
    const response = await this.#apiClient.POST(
      "/api/swap/{id}/collab-refund-evm",
      {
        params: { path: { id: swapId } },
        body,
      },
    );

    if (response.error) {
      throw new Error(
        `Collaborative EVM refund failed: ${response.error.error || "Unknown error"}`,
      );
    }

    return {
      id: response.data.id,
      txHash: response.data.tx_hash,
      message: response.data.message,
    };
  }

  /**
   * Collaborative EVM refund — internal method called by refundSwap.
   * @internal
   */
  async #collabRefundEvm(
    id: string,
    settlement: "swap-back" | "direct" = "direct",
  ): Promise<RefundResult> {
    try {
      const result = await this.collabRefundEvmSwap(id, settlement);
      return {
        success: true,
        message: `${result.message} (tx: ${result.txHash})`,
      };
    } catch (error) {
      return {
        success: false,
        message: `Collaborative EVM refund failed: ${error instanceof Error ? error.message : String(error)}`,
      };
    }
  }

  // =========================================================================
  // Swap Creation - BTC to EVM
  // =========================================================================

  /**
   * Gets the context object for swap creation functions.
   * @internal
   */
  #getCreateContext(): CreateSwapContext {
    return {
      apiClient: this.#apiClient,
      baseUrl: this.#config.baseUrl,
      deriveSwapParams: () => this.deriveSwapParams(),
      evmAddress: this.getEvmAddress(),
      skipKeyIndices: async (n: number) => {
        if (this.#signerStorage) {
          const current = await this.#signerStorage.getKeyIndex();
          await this.#signerStorage.setKeyIndex(current + n);
        }
      },
      storeSwap: (swapId, swapParams, response) =>
        this.#storeSwap(swapId, swapParams, response),
    };
  }

  /**
   * Stores a swap in the configured swap storage.
   * @internal
   */
  async #storeSwap(
    swapId: string,
    swapParams: SwapParams,
    response: Record<string, unknown>,
    targetAddress?: string,
  ): Promise<void> {
    if (!this.#swapStorage) return;

    const storedSwap: StoredSwap = {
      version: SWAP_STORAGE_VERSION,
      swapId,
      keyIndex: swapParams.keyIndex,
      response: response as GetSwapResponse,
      publicKey: bytesToHex(swapParams.publicKey),
      preimage: bytesToHex(swapParams.preimage),
      preimageHash: bytesToHex(swapParams.preimageHash),
      secretKey: bytesToHex(swapParams.secretKey),
      storedAt: Date.now(),
      updatedAt: Date.now(),
      targetAddress,
    };

    await this.#swapStorage.store(storedSwap);
  }

  /**
   * Creates a swap by routing to the correct direction-specific method
   * based on `sourceAsset.chain` and `targetAsset.chain`.
   *
   * Supported directions:
   * - Arkade → EVM
   * - Lightning → EVM
   * - Bitcoin (on-chain) → EVM
   * - Bitcoin (on-chain) → Arkade
   * - EVM → Arkade
   * - EVM → Bitcoin (on-chain)
   * - EVM → Lightning
   *
   * @param options - The swap options including source/target assets, amounts, and addresses.
   * @returns The swap result (response + swapParams).
   * @throws Error if the swap direction is unsupported or required fields are missing.
   */
  async createSwap(options: CreateSwapOptions): Promise<CreateSwapResult> {
    // Resolve source/target from either the new Asset form or the legacy TokenInfo form
    const src = options.source ?? options.sourceAsset;
    const tgt = options.target ?? options.targetAsset;
    if (!src || !tgt) {
      throw new Error(
        "source and target are required (use Asset constants or { chain, tokenId })",
      );
    }

    // Normalize to a common shape with .chain and .token_id
    const sourceAsset: { chain: string; token_id: string } =
      "token_id" in src
        ? { chain: src.chain, token_id: src.token_id }
        : { chain: src.chain, token_id: src.tokenId };
    const targetAsset: { chain: string; token_id: string } =
      "token_id" in tgt
        ? { chain: tgt.chain, token_id: tgt.token_id }
        : { chain: tgt.chain, token_id: tgt.tokenId };

    let sourceChain = sourceAsset.chain;
    let sourceTokenId = sourceAsset.token_id;
    let targetChain = targetAsset.chain;
    let tokenAddress = targetAsset.token_id;

    // If the target is a bridge-only chain (e.g. USDC on Base), automatically
    // remap to the token on Arbitrum for the DEX swap and populate bridgeParams
    // so the backend knows to bridge after. This keeps the remapping logic
    // in one place — SDK consumers just pass their desired target.
    let bridgeParams = options.bridgeParams;
    if (!bridgeParams && isBridgeOnlyChain(targetChain)) {
      const chainName = toChainName(targetChain as Chain);
      if (chainName) {
        const isUsdt0 = Object.values(USDT0_ADDRESSES).some(
          (addr) => addr.toLowerCase() === tokenAddress.toLowerCase(),
        );
        bridgeParams = {
          targetChain: chainName,
          targetTokenAddress: isUsdt0
            ? USDT0_ADDRESSES[chainName]
            : USDC_ADDRESSES[chainName],
        };
        targetChain = "42161"; // Arbitrum
        tokenAddress = isUsdt0
          ? USDT0_ADDRESSES.Arbitrum
          : USDC_ADDRESSES.Arbitrum;
      }
    }

    // Mirror of the outbound remap for CCTP-inbound sources: when the user
    // provides USDC on a chain the backend doesn't accept as a direct swap
    // source (Optimism, Base, Linea, …), rewrite to Arbitrum USDC and
    // populate `inboundBridgeParams` so the backend accounts for the
    // CCTPv2 fast-transfer fee at quote + UserOp-calldata time.
    let inboundBridgeParams = options.inboundBridgeParams;
    const parsedSourceChainId = Number.parseInt(sourceChain, 10);
    if (
      !inboundBridgeParams &&
      !Number.isNaN(parsedSourceChainId) &&
      isCctpOnlySource(parsedSourceChainId)
    ) {
      const source = cctpMetaForChainId(parsedSourceChainId);
      if (sourceTokenId.toLowerCase() !== source.usdc.toLowerCase()) {
        throw new Error(
          `createSwap on ${source.name} requires native USDC (${source.usdc}); got ${sourceTokenId}. Only USDC is bridgeable via CCTP.`,
        );
      }
      inboundBridgeParams = {
        sourceChain: source.name,
        sourceTokenAddress: source.usdc,
      };
      sourceChain = "42161";
      sourceTokenId = USDC_ADDRESSES.Arbitrum;
    }

    // Arkade → EVM
    if (isArkade(sourceAsset) && isEvmToken(targetChain)) {
      return this.createArkadeToEvmSwapGeneric({
        targetAddress: options.targetAddress,
        tokenAddress,
        evmChainId: Number(targetChain),
        sourceAmount: options.sourceAmount
          ? BigInt(options.sourceAmount)
          : undefined,
        targetAmount: options.targetAmount
          ? BigInt(options.targetAmount)
          : undefined,
        referralCode: options.referralCode,
        bridgeParams,
      });
    }

    // Lightning → EVM
    if (isLightning(sourceAsset) && isEvmToken(targetChain)) {
      return this.createLightningToEvmSwapGeneric({
        targetAddress: options.targetAddress,
        tokenAddress,
        evmChainId: Number(targetChain),
        amountIn: options.sourceAmount,
        amountOut: options.targetAmount,
        referralCode: options.referralCode,
        bridgeParams,
      });
    }

    // Arkade → Lightning
    if (isArkade(sourceAsset) && isLightning(targetAsset)) {
      // Detect whether targetAddress is a Lightning address (user@domain),
      // an LNURL (lnurl1...), or a BOLT11 invoice (starts with ln...).
      const isAddress = /^[a-zA-Z0-9._-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$/.test(
        options.targetAddress,
      );
      const isLnurlStr = /^lnurl1[a-z0-9]+$/i.test(options.targetAddress);

      if (isAddress || isLnurlStr) {
        if (
          options.targetAmount == null ||
          !Number.isFinite(options.targetAmount) ||
          options.targetAmount <= 0
        ) {
          throw new Error(
            "targetAmount (in sats) is required when using a Lightning address or LNURL",
          );
        }
        return this.createArkadeToLightningSwap({
          ...(isAddress
            ? { lightningAddress: options.targetAddress }
            : { lnurl: options.targetAddress }),
          amountSats: options.targetAmount,
          referralCode: options.referralCode,
        });
      }

      return this.createArkadeToLightningSwap({
        lightningInvoice: options.targetAddress,
        referralCode: options.referralCode,
      });
    }

    // Lightning → Arkade
    if (isLightning(sourceAsset) && isArkade(targetAsset)) {
      if (options.targetAmount == null) {
        throw new Error(
          "targetAmount (sats to receive on Arkade) is required for Lightning → Arkade swaps",
        );
      }
      return this.createLightningToArkadeSwap({
        satsReceive: options.targetAmount,
        targetAddress: options.targetAddress,
        referralCode: options.referralCode,
      });
    }

    // Bitcoin (on-chain) → EVM
    if (isBtcOnchain(sourceAsset) && isEvmToken(targetChain)) {
      return this.createBitcoinToEvmSwap({
        targetAddress: options.targetAddress,
        tokenAddress,
        evmChainId: Number(targetChain),
        sourceAmount: options.sourceAmount,
        targetAmount: options.targetAmount,
        referralCode: options.referralCode,
        bridgeParams,
      });
    }

    // Bitcoin (on-chain) → Arkade
    if (isBtcOnchain(sourceAsset) && isArkade(targetAsset)) {
      if (options.targetAmount == null) {
        throw new Error(
          "targetAmount (sats to receive on Arkade) is required for Bitcoin → Arkade swaps",
        );
      }
      return this.createBitcoinToArkadeSwap({
        satsReceive: options.targetAmount,
        targetAddress: options.targetAddress,
        referralCode: options.referralCode,
      });
    }

    // EVM → Arkade
    if (isSourceEvmChain(sourceChain) && isArkade(targetAsset)) {
      if (!options.userAddress && !options.gasless) {
        throw new Error(
          "userAddress is required for EVM → Arkade swaps (unless gasless)",
        );
      }
      return this.createEvmToArkadeSwapGeneric({
        targetAddress: options.targetAddress,
        tokenAddress: sourceTokenId,
        evmChainId: Number(sourceChain),
        userAddress: options.userAddress ?? "",
        sourceAmount: options.sourceAmount
          ? BigInt(options.sourceAmount)
          : undefined,
        targetAmount: options.targetAmount,
        referralCode: options.referralCode,
        gasless: options.gasless,
        inboundBridgeParams,
      });
    }

    // EVM → Bitcoin (on-chain)
    if (isSourceEvmChain(sourceChain) && isBtcOnchain(targetAsset)) {
      if (!options.userAddress && !options.gasless) {
        throw new Error(
          "userAddress is required for EVM → Bitcoin swaps (unless gasless)",
        );
      }
      return this.createEvmToBitcoinSwap({
        tokenAddress: sourceTokenId,
        evmChainId: Number(sourceChain),
        userAddress: options.userAddress ?? "",
        targetAddress: options.targetAddress,
        sourceAmount: options.sourceAmount
          ? BigInt(options.sourceAmount)
          : undefined,
        targetAmount: options.targetAmount,
        referralCode: options.referralCode,
        gasless: options.gasless,
        inboundBridgeParams,
      });
    }

    // EVM → Lightning
    if (isSourceEvmChain(sourceChain) && isLightning(targetAsset)) {
      if (!options.userAddress && !options.gasless) {
        throw new Error(
          "userAddress is required for EVM → Lightning swaps (unless gasless)",
        );
      }

      // Detect whether targetAddress is a Lightning address (user@domain),
      // an LNURL (lnurl1...), or a BOLT11 invoice (starts with ln...).
      const isAddress = /^[a-zA-Z0-9._-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$/.test(
        options.targetAddress,
      );
      const isLnurlStr = /^lnurl1[a-z0-9]+$/i.test(options.targetAddress);

      if (isAddress || isLnurlStr) {
        if (
          options.targetAmount == null ||
          !Number.isFinite(options.targetAmount) ||
          options.targetAmount <= 0
        ) {
          throw new Error(
            "targetAmount (in sats) is required when using a Lightning address or LNURL",
          );
        }
        return this.createEvmToLightningSwapGeneric({
          ...(isAddress
            ? { lightningAddress: options.targetAddress }
            : { lnurl: options.targetAddress }),
          amountSats: options.targetAmount,
          evmChainId: Number(sourceChain),
          tokenAddress: sourceTokenId,
          userAddress: options.userAddress ?? "",
          referralCode: options.referralCode,
          gasless: options.gasless,
          inboundBridgeParams,
        });
      }

      if (isArkade(targetAsset) && isLightning(sourceAsset)) {
        if (!options.targetAmount) {
          throw new Error("Target amount must be set");
        }

        return this.createLightningToArkadeSwap({
          targetAddress: options.targetAddress,
          referralCode: options.referralCode,
          satsReceive: options.targetAmount,
        });
      }

      return this.createEvmToLightningSwapGeneric({
        lightningInvoice: options.targetAddress,
        evmChainId: Number(sourceChain),
        tokenAddress: sourceTokenId,
        userAddress: options.userAddress ?? "",
        referralCode: options.referralCode,
        gasless: options.gasless,
        inboundBridgeParams,
      });
    }

    throw new Error(
      `Unsupported swap direction: ${sourceChain} → ${targetChain}`,
    );
  }

  /**
   * Creates a new Arkade-to-EVM swap via the generic chain-agnostic endpoint.
   *
   * Uses the `/swap/arkade/evm` endpoint which supports any ERC-20 token
   * reachable through 1inch aggregation. Returns coordinator address and
   * optional 1inch calldata for the redeem-and-swap flow.
   *
   * @param options - The swap options.
   * @returns The swap response and parameters for storage.
   * @throws Error if the swap creation fails.
   *
   * @example
   * ```ts
   * const result = await client.createArkadeToEvmSwapGeneric({
   *   targetAddress: "0x1234...",
   *   tokenAddress: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", // USDC on Polygon
   *   evmChainId: 137,
   *   sourceAmount: 100000, // 100k sats
   * });
   * console.log("Fund:", result.response.btc_vhtlc_address);
   * console.log("Coordinator:", result.response.evm_coordinator_address);
   * ```
   */
  async createArkadeToEvmSwapGeneric(
    options: ArkadeToEvmSwapOptions,
  ): Promise<ArkadeToEvmSwapResult> {
    return createArkadeToEvmSwapGeneric(options, this.#getCreateContext());
  }

  /**
   * Creates a new Lightning to EVM swap using the generic chain-agnostic endpoint.
   *
   * @param options - The swap options including evmChainId and tokenAddress.
   * @returns The swap response and parameters for storage.
   */
  async createLightningToEvmSwapGeneric(
    options: LightningToEvmSwapGenericOptions,
  ): Promise<LightningToEvmSwapGenericResult> {
    return createLightningToEvmSwapGeneric(options, this.#getCreateContext());
  }

  /**
   * Creates a new Bitcoin (on-chain) to EVM swap.
   *
   * Automatically derives swap parameters and increments the key index.
   *
   * @param options - The swap options.
   * @returns The swap response and parameters for storage.
   * @throws Error if the swap creation fails.
   *
   * @example
   * ```ts
   * const result = await client.createBitcoinToEvmSwap({
   *   targetAddress: "0x1234...",
   *   tokenAddress: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", // USDC on Polygon
   *   evmChainId: 137,
   *   sourceAmount: 100000, // 100k sats
   * });
   * console.log("Send BTC to:", result.response.btc_htlc_address);
   * ```
   */
  async createBitcoinToEvmSwap(
    options: BitcoinToEvmSwapOptions,
  ): Promise<BitcoinToEvmSwapResult> {
    return createBitcoinToEvmSwap(options, this.#getCreateContext());
  }

  // =========================================================================
  // Swap Creation - Bitcoin (on-chain) to Arkade
  // =========================================================================

  /**
   * Creates a new Bitcoin (on-chain) to Arkade swap.
   *
   * The user sends on-chain BTC to a Taproot HTLC address and receives
   * Arkade VTXOs after the server funds the Arkade VHTLC.
   *
   * Automatically derives swap parameters and increments the key index.
   *
   * @param options - The swap options.
   * @returns The swap response and parameters for storage.
   * @throws Error if the swap creation fails.
   *
   * @example
   * ```ts
   * const result = await client.createBitcoinToArkadeSwap({
   *   satsReceive: 100000, // 100k sats to receive on Arkade
   *   targetAddress: "ark1q...", // Arkade address
   * });
   * console.log("Send BTC to:", result.response.btc_htlc_address);
   * console.log("Amount to send:", result.response.source_amount, "sats");
   * ```
   */
  async createBitcoinToArkadeSwap(
    options: BitcoinToArkadeSwapOptions,
  ): Promise<BitcoinToArkadeSwapResult> {
    return createBitcoinToArkadeSwap(options, this.#getCreateContext());
  }

  // =========================================================================
  // Swap Creation - Lightning to Arkade
  // =========================================================================

  /**
   * Creates a new Lightning to Arkade swap.
   *
   * The user pays a Lightning invoice and receives Arkade VTXOs
   * after the server funds the Arkade VHTLC.
   *
   * @param options - The swap options.
   * @returns The swap response and parameters for storage.
   * @throws Error if the swap creation fails.
   *
   * @example
   * ```ts
   * const result = await client.createLightningToArkadeSwap({
   *   satsReceive: 100000, // 100k sats to receive on Arkade
   *   targetAddress: "ark1q...", // Arkade address
   * });
   * console.log("Pay this invoice:", result.response.bolt11_invoice);
   * ```
   */
  async createLightningToArkadeSwap(
    options: LightningToArkadeSwapOptions,
  ): Promise<LightningToArkadeSwapResult> {
    return createLightningToArkadeSwap(options, this.#getCreateContext());
  }

  // =========================================================================
  // Swap Creation - Arkade to Lightning
  // =========================================================================

  /**
   * Creates a new Arkade to Lightning swap.
   *
   * The user sends Arkade VTXOs and a Lightning invoice gets paid
   * via the Lendaswap server.
   *
   * @param options - The swap options.
   * @returns The swap response and parameters for storage.
   * @throws Error if the swap creation fails.
   *
   * @example
   * ```ts
   * const result = await client.createArkadeToLightningSwap({
   *   lightningInvoice: "lnbc100u1p...",
   * });
   * console.log("Fund:", result.response.arkade_vhtlc_address);
   * console.log("Amount:", result.response.source_amount, "sats");
   * ```
   */
  async createArkadeToLightningSwap(
    options: ArkadeToLightningSwapOptions,
  ): Promise<ArkadeToLightningSwapResult> {
    return createArkadeToLightningSwap(options, this.#getCreateContext());
  }

  // =========================================================================
  // Arkade-to-Lightning: Fee Estimation & Retry
  // =========================================================================

  /**
   * Calculate the correct Lightning invoice amount for an Arkade→Lightning swap.
   *
   * Given the source amount in sats (what will be locked in the VHTLC), returns
   * the target amount that the Lightning invoice should be for (after fees
   * are deducted).
   *
   * Useful for:
   * - Knowing what invoice amount to generate before creating a swap
   * - Retrying a failed swap: the user's funds are locked in the old VHTLC at
   *   `sourceAmountSats`, and the new invoice must match the expected target amount
   *
   * @param sourceAmountSats - Amount in sats that will fund the VHTLC
   * @returns Quote with target amount, fee breakdown, and exchange rate
   * @throws Error if the quote request fails or the amount is out of range
   *
   * @example
   * ```ts
   * const quote = await client.getArkadeToLightningQuote(100000);
   * console.log(`Invoice should be for ${quote.target_amount} sats`);
   * console.log(`Fees: ${quote.fee} sats`);
   * ```
   */
  async getArkadeToLightningQuote(
    sourceAmountSats: number,
  ): Promise<QuoteResponse> {
    const { data, error } = await this.#apiClient.GET("/quote", {
      params: {
        query: {
          source_chain: "Arkade",
          source_token: "btc",
          target_chain: "Lightning",
          target_token: "btc",
          source_amount: sourceAmountSats,
        },
      },
    });

    if (error) {
      throw new Error(
        `Failed to get Arkade→Lightning quote: ${typeof error === "string" ? error : JSON.stringify(error)}`,
      );
    }
    if (!data) {
      throw new Error("No quote data returned");
    }
    return data;
  }

  /**
   * Retry a failed Arkade→Lightning swap with a new Lightning invoice or LNURL.
   *
   * When an Arkade→Lightning swap fails (status `serverwontfund` or `clientinvalidfunded`),
   * this method:
   * 1. Creates a new Arkade→Lightning swap with the new invoice/LNURL
   * 2. Collaboratively refunds the old VHTLC into the new swap's VHTLC
   *
   * The refund uses the collaborative `refund` script leaf (3-of-3: sender + receiver + Arkade),
   * which is instant (no locktime wait). The receiver cooperates because the swap is in
   * `invoice.failedToPay` state.
   *
   * **Invoice amount**: The new invoice must match the expected target amount for the
   * source amount locked in the old VHTLC. Use {@link getArkadeToLightningQuote} to
   * calculate the correct invoice amount, or use `lightningAddress` (LNURL) which
   * handles amount negotiation automatically.
   *
   * @param swapId - ID of the failed swap (must be in `serverwontfund` or `clientinvalidfunded` status)
   * @param options - New invoice or Lightning address for the retry
   * @returns The new swap response and the refund transaction ID
   * @throws Error if the swap is not in the right state, amount mismatch, or server refuses
   *
   * @example
   * ```ts
   * // With LNURL (recommended — handles amount automatically):
   * const result = await client.retryArkadeToLightningSwap(swapId, {
   *   lightningAddress: "user@speed.app",
   * });
   *
   * // With invoice (must match expected amount):
   * const quote = await client.getArkadeToLightningQuote(oldSwap.boltz_amount_sats);
   * // Generate invoice for quote.target_amount sats, then:
   * const result = await client.retryArkadeToLightningSwap(swapId, {
   *   lightningInvoice: "lnbc...",
   * });
   * ```
   */
  async retryArkadeToLightningSwap(
    swapId: string,
    options: {
      /** BOLT11 Lightning invoice. Must be for the correct amount (use getArkadeToLightningQuote). */
      lightningInvoice?: string;
      /** Lightning address (LNURL). Amount is negotiated automatically. */
      lightningAddress?: string;
    },
  ): Promise<{
    /** The new swap */
    newSwap: ArkadeToLightningSwapResponse;
    /** Transaction ID of the collaborative refund from old → new VHTLC */
    refundTxId: string;
    /** Amount refunded in sats */
    refundAmount: bigint;
  }> {
    if (!this.#swapStorage) {
      throw new Error(
        "Swap storage not configured. Cannot retrieve keys needed for refund.",
      );
    }

    if (!options.lightningInvoice && !options.lightningAddress) {
      throw new Error(
        "Provide either lightningInvoice or lightningAddress for retry",
      );
    }
    if (options.lightningInvoice && options.lightningAddress) {
      throw new Error(
        "Provide either lightningInvoice or lightningAddress, not both",
      );
    }

    // 1. Validate the old swap is in a retryable state
    const oldSwap = await this.getSwap(swapId, {
      updateStorage: true,
    });

    if (oldSwap.direction !== "arkade_to_lightning") {
      throw new Error(
        `Expected arkade_to_lightning swap, got ${oldSwap.direction}`,
      );
    }

    const retryableStatuses = ["serverwontfund", "clientinvalidfunded"];
    if (!retryableStatuses.includes(oldSwap.status)) {
      throw new Error(
        `Swap must be in serverwontfund or clientinvalidfunded status to retry ` +
          `(current: ${oldSwap.status}). ` +
          (oldSwap.status === "serverredeemed"
            ? "This swap completed successfully — no retry needed."
            : oldSwap.status === "clientrefunded"
              ? "This swap was already refunded."
              : "The swap may still be in progress."),
      );
    }

    // 2. Get the source amount locked in the old VHTLC
    const sourceAmountSats = oldSwap.boltz_amount_sats;

    // 3. Build create-swap options
    const createOptions: ArkadeToLightningSwapOptions = {};

    if (options.lightningAddress) {
      // LNURL: use the quote to determine the right amount
      const quote = await this.getArkadeToLightningQuote(sourceAmountSats);
      createOptions.lightningAddress = options.lightningAddress;
      createOptions.amountSats = Number(quote.target_amount);
    } else if (options.lightningInvoice) {
      createOptions.lightningInvoice = options.lightningInvoice;
    }

    // 4. Create the new swap
    const newSwapResult = await this.createArkadeToLightningSwap(createOptions);
    const newSwap = newSwapResult.response;

    // 5. Verify the new swap's expected funding amount matches our old VHTLC
    const newExpectedAmount = newSwap.boltz_amount_sats;
    if (newExpectedAmount !== sourceAmountSats) {
      const mismatchHint =
        newExpectedAmount > sourceAmountSats
          ? "The invoice amount may be too high"
          : "The invoice amount may be too low";

      throw new Error(
        `Amount mismatch: new swap expects ${newExpectedAmount} sats in VHTLC ` +
          `but old VHTLC has ${sourceAmountSats} sats. ` +
          `${mismatchHint} — use getArkadeToLightningQuote(${sourceAmountSats}) ` +
          `to calculate the correct invoice amount before retrying.`,
      );
    }

    const storedSwap = await this.#swapStorage.get(swapId);
    if (!storedSwap) {
      throw new Error(
        `Swap ${swapId} not found in local storage. ` +
          `The secret key is required to sign the collaborative refund.`,
      );
    }

    const fullPubKey = storedSwap.publicKey;
    const userPubKey =
      fullPubKey.length === 66 ? fullPubKey.slice(2) : fullPubKey;

    const hashLock = oldSwap.hash_lock.startsWith("0x")
      ? oldSwap.hash_lock.slice(2)
      : oldSwap.hash_lock;

    // Collaborative refund: old VHTLC → new VHTLC address
    console.log("[retry] Starting collaborative refund", {
      oldSwapId: swapId,
      newSwapId: newSwap.id,
      sourceAmount: sourceAmountSats,
      network: oldSwap.network,
    });

    const refundResult = await collabRefundArkadeToLightningOffchain({
      userSecretKey: storedSwap.secretKey,
      userPubKey,
      receiverPubKey: oldSwap.receiver_pk,
      arkadeServerPubKey: oldSwap.arkade_server_pk,
      hashLock,
      vhtlcAddress: oldSwap.arkade_vhtlc_address,
      refundLocktime: oldSwap.vhtlc_refund_locktime,
      unilateralClaimDelay: oldSwap.unilateral_claim_delay,
      unilateralRefundDelay: oldSwap.unilateral_refund_delay,
      unilateralRefundWithoutReceiverDelay:
        oldSwap.unilateral_refund_without_receiver_delay,
      destinationAddress: newSwap.arkade_vhtlc_address,
      network: oldSwap.network,
      arkadeServerUrl: this.#config.arkadeServerUrl,
      swapId,
      apiClient: this.#apiClient,
    });

    return {
      newSwap,
      refundTxId: refundResult.txId,
      refundAmount: refundResult.refundAmount,
    };
  }

  // =========================================================================
  // Swap Creation - EVM to Arkade
  // =========================================================================

  /**
   * Creates a new EVM-to-Arkade swap via the generic endpoint.
   *
   * Uses the chain-agnostic `/swap/evm/arkade` endpoint which supports any
   * ERC-20 token reachable through 1inch aggregation.
   *
   * @param options - The swap options.
   * @returns The swap response and parameters for storage.
   * @throws Error if the swap creation fails.
   *
   * @example
   * ```ts
   * const result = await client.createEvmToArkadeSwapGeneric({
   *   targetAddress: "ark1q...",
   *   tokenAddress: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", // USDC on Polygon
   *   evmChainId: 137,
   *   userAddress: "0x1234...",
   *   sourceAmount: 100000000, // 100 USDC (6 decimals)
   * });
   * console.log("HTLC:", result.response.evm_htlc_address);
   * ```
   */
  async createEvmToArkadeSwapGeneric(
    options: EvmToArkadeSwapGenericOptions,
  ): Promise<EvmToArkadeSwapGenericResult> {
    return createEvmToArkadeSwapGeneric(options, this.#getCreateContext());
  }

  /**
   * Creates a new EVM-to-Bitcoin (on-chain) swap.
   *
   * Uses the chain-agnostic `/swap/evm/bitcoin` endpoint which supports any
   * ERC-20 token reachable through 1inch aggregation. The user locks tokens
   * in an EVM HTLC and receives BTC to an on-chain Taproot HTLC.
   *
   * @param options - The swap options.
   * @returns The swap response and parameters for storage.
   * @throws Error if the swap creation fails.
   *
   * @example
   * ```ts
   * const result = await client.createEvmToBitcoinSwap({
   *   tokenAddress: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", // USDC on Polygon
   *   evmChainId: 137,
   *   userAddress: "0x1234...",
   *   sourceAmount: 100000000n, // 100 USDC (6 decimals)
   * });
   * console.log("EVM HTLC:", result.response.evm_htlc_address);
   * console.log("BTC HTLC:", result.response.btc_htlc_address);
   * ```
   */
  async createEvmToBitcoinSwap(
    options: EvmToBitcoinSwapOptions,
  ): Promise<EvmToBitcoinSwapResult> {
    return createEvmToBitcoinSwap(options, this.#getCreateContext());
  }

  /**
   * Creates a new EVM to Lightning swap using the chain-agnostic generic endpoint.
   *
   * This allows users to swap any ERC-20 token from any supported EVM chain
   * to pay a Lightning invoice.
   *
   * @param options - The swap options including Lightning invoice, chain ID, and token address.
   * @returns The swap response and parameters for storage.
   * @throws Error if the swap creation fails.
   *
   * @example
   * ```ts
   * const result = await client.createEvmToLightningSwapGeneric({
   *   lightningInvoice: "lnbc...",
   *   evmChainId: 137, // Polygon
   *   tokenAddress: "0x1BFD67037B42Cf73acF2047067bd4F2C47D9BfD6", // WBTC
   *   userAddress: "0x1234...",
   * });
   * console.log("HTLC contract:", result.response.evm_htlc_address);
   * console.log("Swap ID:", result.response.id);
   * ```
   */
  async createEvmToLightningSwapGeneric(
    options: EvmToLightningSwapGenericOptions,
  ): Promise<EvmToLightningSwapGenericResult> {
    return createEvmToLightningSwapGeneric(options, this.#getCreateContext());
  }

  // =========================================================================
  // Coordinator Funding (EVM-to-BTC via DEX + HTLC)
  // =========================================================================

  /**
   * Gets Permit2-based call data to fund an EVM-to-BTC swap via the HTLCCoordinator.
   *
   * Uses Permit2 for gasless token approval: the user signs an off-chain EIP-712
   * message instead of submitting a separate `approve` tx to the source token.
   *
   * The returned `approve` is a one-time max approval of the source token to the
   * Permit2 contract (can be skipped if already approved). The `executeAndCreate`
   * calldata includes the Permit2 signature and can be submitted by anyone (relayer).
   *
   * @param swapId - The UUID of the swap
   * @param chainId - The EVM chain ID (e.g. 137 for Polygon, 1 for Ethereum)
   * @returns The approve (token → Permit2) and executeAndCreateWithPermit2 call data
   *
   * @example
   * ```ts
   * const funding = await client.getCoordinatorFundingCallDataPermit2(swap.response.id, 137);
   *
   * // Step 1: One-time approve source token to Permit2 (can skip if already done)
   * await wallet.sendTransaction({ to: funding.approve.to, data: funding.approve.data });
   *
   * // Step 2: Execute swap + create HTLC (signed via Permit2)
   * await wallet.sendTransaction({ to: funding.executeAndCreate.to, data: funding.executeAndCreate.data });
   * ```
   */
  async getCoordinatorFundingCallDataPermit2(
    swapId: string,
    chainId: number,
  ): Promise<Permit2SignedFundingCallData> {
    // 1. Look up stored swap to get the secret key for signing
    const storedSwap = await this.getStoredSwap(swapId);
    if (!storedSwap) {
      throw new Error(
        `Swap ${swapId} not found in local storage. Cannot sign Permit2 message without the secret key.`,
      );
    }

    const swap = await this.getSwap(swapId);

    if (
      swap.direction !== "evm_to_arkade" &&
      swap.direction !== "evm_to_bitcoin" &&
      swap.direction !== "evm_to_lightning"
    ) {
      throw new Error(
        `Expected evm_to_arkade/evm_to_bitcoin/evm_to_lightning swap, got ${swap.direction}. Permit2 fund method is for EVM-sourced swaps.`,
      );
    }

    // 2. Fetch Permit2 funding data from server
    const baseUrl = this.#config.baseUrl.replace(/\/$/, "");
    const url = `${baseUrl}/swap/${swapId}/swap-and-lock-calldata-permit2`;
    const headers: Record<string, string> = {};
    if (this.#config.orgCode) {
      headers["X-Org-Code"] = this.#config.orgCode;
    }

    const resp = await fetch(url, { headers });
    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(
        `Failed to get Permit2 funding data: ${resp.status} ${body}`,
      );
    }

    const serverData = (await resp.json()) as {
      coordinator_address: string;
      permit2_address: string;
      source_token_address: string;
      source_amount: string;
      lock_token_address: string;
      preimage_hash: string;
      claim_address: string;
      timelock: number;
      calls: Array<{ target: string; value: string; call_data: string }>;
      calls_hash: string;
    };

    // 3. Generate random Permit2 nonce and deadline
    const nonceBytes = new Uint8Array(32);
    crypto.getRandomValues(nonceBytes);
    const nonce = BigInt(
      `0x${Array.from(nonceBytes)
        .map((b) => b.toString(16).padStart(2, "0"))
        .join("")}`,
    );
    const deadline = BigInt(Math.floor(Date.now() / 1000) + 30 * 60); // 30 minutes

    const sourceAmount = BigInt(serverData.source_amount);

    // 4. Build EIP-712 digest
    // refundAddress = coordinator address for overload 1 (depositor tracking)
    const digest = buildPermit2FundingDigest({
      chainId,
      coordinatorAddress: serverData.coordinator_address,
      sourceToken: serverData.source_token_address,
      sourceAmount,
      preimageHash: serverData.preimage_hash,
      lockToken: serverData.lock_token_address,
      claimAddress: serverData.claim_address,
      refundAddress: serverData.coordinator_address,
      timelock: serverData.timelock,
      callsHash: serverData.calls_hash,
      nonce,
      deadline,
    });

    // 5. Sign with the EVM key (deterministic for new swaps, per-swap for legacy)
    const evmKey = this.#getEvmSigningKey();
    const sig = signEvmDigest(evmKey, digest);
    // Compact signature: r (32 bytes) || s (32 bytes) || v (1 byte)
    const rClean = sig.r.replace(/^0x/, "");
    const sClean = sig.s.replace(/^0x/, "");
    const vHex = sig.v.toString(16).padStart(2, "0");
    const compactSignature = `0x${rClean}${sClean}${vHex}`;

    // 6. Build calls array for the coordinator
    const calls = serverData.calls.map((c) => ({
      target: c.target,
      value: BigInt(c.value),
      data: c.call_data,
    }));

    // Derive depositor address from the EVM signing key
    const depositorAddress = deriveEvmAddress(evmKey);

    // 7. Encode executeAndCreateWithPermit2 calldata
    const encoded = encodeExecuteAndCreateWithPermit2(
      serverData.coordinator_address,
      {
        calls,
        preimageHash: serverData.preimage_hash,
        token: serverData.lock_token_address,
        claimAddress: serverData.claim_address,
        timelock: serverData.timelock,
        depositor: depositorAddress,
        sourceToken: serverData.source_token_address,
        sourceAmount,
        nonce,
        deadline,
        signature: compactSignature,
      },
    );

    // 8. Build approve: source token → Permit2 (max uint256, one-time)
    const maxUint256 = BigInt(
      "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    );
    const approve = encodeApproveCallData(
      serverData.source_token_address,
      PERMIT2_ADDRESS,
      maxUint256,
    );

    return {
      approve: {
        to: approve.to,
        data: approve.data,
      },
      executeAndCreate: {
        to: encoded.to,
        data: encoded.data,
      },
    };
  }

  /**
   * Get unsigned Permit2 funding parameters for the sovereign flow.
   *
   * Returns the EIP-712 typed data structure and all parameters needed
   * for the user's browser wallet to sign via `signTypedData` and then
   * encode + submit the `executeAndCreateWithPermit2` transaction.
   *
   * Unlike `getCoordinatorFundingCallDataPermit2`, this method does NOT
   * sign the Permit2 message — the caller is responsible for obtaining
   * the signature from the user's wallet.
   *
   * @param swapId - The UUID of the swap
   * @param chainId - The EVM chain ID
   * @returns Unsigned Permit2 funding data including typed data for signing
   *
   * @example
   * ```ts
   * const params = await client.getPermit2FundingParamsUnsigned(swapId, chainId);
   *
   * // In the browser with wagmi/viem:
   * const signature = await walletClient.signTypedData(params.typedData);
   *
   * // Encode the final calldata
   * const calldata = encodeExecuteAndCreateWithPermit2(
   *   params.coordinatorAddress,
   *   {
   *     calls: params.calls,
   *     preimageHash: params.preimageHash,
   *     token: params.lockTokenAddress,
   *     claimAddress: params.claimAddress,
   *     timelock: params.timelock,
   *     depositor: userWalletAddress,
   *     sourceToken: params.sourceTokenAddress,
   *     sourceAmount: params.sourceAmount,
   *     nonce: params.nonce,
   *     deadline: params.deadline,
   *     signature,
   *   },
   * );
   * ```
   */
  async getPermit2FundingParamsUnsigned(
    swapId: string,
    chainId: number,
  ): Promise<UnsignedPermit2FundingData> {
    const swap = await this.getSwap(swapId);

    if (
      swap.direction !== "evm_to_arkade" &&
      swap.direction !== "evm_to_bitcoin" &&
      swap.direction !== "evm_to_lightning"
    ) {
      throw new Error(
        `Expected evm_to_arkade/evm_to_bitcoin/evm_to_lightning swap, got ${swap.direction}. Permit2 fund method is for EVM-sourced swaps.`,
      );
    }

    // Fetch Permit2 funding data from server
    const baseUrl = this.#config.baseUrl.replace(/\/$/, "");
    const url = `${baseUrl}/swap/${swapId}/swap-and-lock-calldata-permit2`;
    const headers: Record<string, string> = {};
    if (this.#config.orgCode) {
      headers["X-Org-Code"] = this.#config.orgCode;
    }

    const resp = await fetch(url, { headers });
    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(
        `Failed to get Permit2 funding data: ${resp.status} ${body}`,
      );
    }

    const serverData = (await resp.json()) as {
      coordinator_address: string;
      permit2_address: string;
      source_token_address: string;
      source_amount: string;
      lock_token_address: string;
      preimage_hash: string;
      claim_address: string;
      timelock: number;
      calls: Array<{ target: string; value: string; call_data: string }>;
      calls_hash: string;
    };

    // Generate random Permit2 nonce and deadline
    const nonceBytes = new Uint8Array(32);
    crypto.getRandomValues(nonceBytes);
    const nonce = BigInt(
      `0x${Array.from(nonceBytes)
        .map((b) => b.toString(16).padStart(2, "0"))
        .join("")}`,
    );
    const deadline = BigInt(Math.floor(Date.now() / 1000) + 30 * 60); // 30 minutes

    const sourceAmount = BigInt(serverData.source_amount);

    const calls = serverData.calls.map((c) => ({
      target: c.target,
      value: BigInt(c.value),
      data: c.call_data,
    }));

    // Build EIP-712 typed data for wallet signing
    const fundingParams = {
      chainId,
      coordinatorAddress: serverData.coordinator_address,
      sourceToken: serverData.source_token_address,
      sourceAmount,
      preimageHash: serverData.preimage_hash,
      lockToken: serverData.lock_token_address,
      claimAddress: serverData.claim_address,
      refundAddress: serverData.coordinator_address, // overload 1: depositor tracking
      timelock: serverData.timelock,
      callsHash: serverData.calls_hash,
      nonce,
      deadline,
    };

    const typedData = buildPermit2TypedData(fundingParams);

    return {
      coordinatorAddress: serverData.coordinator_address,
      sourceTokenAddress: serverData.source_token_address,
      sourceAmount,
      lockTokenAddress: serverData.lock_token_address,
      preimageHash: serverData.preimage_hash,
      claimAddress: serverData.claim_address,
      timelock: serverData.timelock,
      calls,
      callsHash: serverData.calls_hash,
      nonce,
      deadline,
      typedData,
    };
  }

  /**
   * Fund an EVM-sourced swap using an external wallet (e.g. MetaMask).
   *
   * Handles the full flow:
   * 1. Check ERC-20 allowance to Permit2 — approve if insufficient
   * 2. Sign the Permit2 EIP-712 typed data
   * 3. Encode and send the `executeAndCreateWithPermit2` transaction
   * 4. Wait for the transaction receipt
   *
   * @param swapId - The UUID of the swap
   * @param signer - An {@link EvmSigner} wrapping the user's wallet
   * @param options - Optional CCTP-path tuning: `maxFee` caps the
   *                  CCTP fast-transfer fee (defaults to the
   *                  IRIS-quoted value); `onProgress` receives
   *                  per-phase updates during the CCTP flow;
   *                  `signal` aborts the attestation wait. All
   *                  three are ignored when the swap takes the
   *                  direct-Permit2 path.
   * @returns The funding transaction hash, plus an optional `cctp`
   *          object with `burnTxHash`, `userOpHash`, and
   *          `smartAccountAddress` when the CCTP path ran.
   *
   * @example
   * ```ts
   * // Wrap wagmi/viem into an EvmSigner
   * const signer: EvmSigner = {
   *   address: walletClient.account.address,
   *   chainId: walletClient.chain.id,
   *   signTypedData: (td) => walletClient.signTypedData({ ...td, account: walletClient.account }),
   *   sendTransaction: (tx) => walletClient.sendTransaction({ to: tx.to, data: tx.data, chain, gas: tx.gas }),
   *   waitForReceipt: (hash) => publicClient.waitForReceipt({ hash }),
   *   getTransaction: (hash) => publicClient.getTransaction({ hash }),
   *   call: (tx) => publicClient.call(tx),
   * };
   *
   * const { txHash } = await client.fundSwap(swapId, signer);
   * ```
   */
  async fundSwap(
    swapId: string,
    signer: EvmSigner,
    options?: {
      /** CCTP fast-transfer fee cap in USDC units. Only consulted on
       *  the CCTP path; defaults to the IRIS-quoted fee. */
      maxFee?: bigint;
      /** Progress callback for the CCTP flow. Ignored on direct path. */
      onProgress?: (step: CctpProgressStep) => void;
      /** Abort signal — cancels the CCTP attestation wait. */
      signal?: AbortSignal;
    },
  ): Promise<{ txHash: string; cctp?: CctpFundSwapResult }> {
    // Dispatch the CCTP-inbound path only for chains the backend does
    // NOT accept as a direct swap source (Optimism, Base, Linea, …).
    // Ethereum / Polygon / Arbitrum fund via Permit2 on the source
    // chain even though they're CCTP-supported. Source asset must be
    // native USDC for the chain — otherwise CCTP isn't viable and we
    // surface a clear error instead of silently burning the wrong
    // token through TokenMessenger.
    if (isCctpOnlySource(signer.chainId)) {
      const stored = await this.#swapStorage?.get(swapId);
      const swapResponse =
        stored?.response ??
        (await this.getSwap(swapId, { updateStorage: true }));
      const source = cctpMetaForChainId(signer.chainId);

      // CCTP-inbound swaps are stored against post-hop Arbitrum USDC
      // (`source_token` reports the Arbitrum address), so we validate
      // against the `bridge_source_*` fields the backend sets when the
      // swap was created through the CCTP path. If those are missing
      // the swap wasn't created for a CCTP-inbound flow — reject rather
      // than silently burn USDC through the TokenMessenger.
      const bridgeSourceChain = (
        swapResponse as { bridge_source_chain?: string }
      ).bridge_source_chain;
      const bridgeSourceToken = (
        swapResponse as { bridge_source_token_address?: string }
      ).bridge_source_token_address;
      if (!bridgeSourceChain || !bridgeSourceToken) {
        throw new Error(
          `fundSwap on chain ${signer.chainId} expects a CCTP-inbound swap but the swap has no bridge_source_chain set. ` +
            `Was the swap created against native USDC on a CCTP-only chain?`,
        );
      }
      if (bridgeSourceChain !== source.name) {
        throw new Error(
          `fundSwap signer is on ${source.name} but the swap was created for bridge source ${bridgeSourceChain}.`,
        );
      }
      if (bridgeSourceToken.toLowerCase() !== source.usdc.toLowerCase()) {
        throw new Error(
          `fundSwap on ${source.name} requires native USDC (${source.usdc}), but swap's bridge_source_token_address is ${bridgeSourceToken}. ` +
            `Only native USDC is bridgeable via CCTP.`,
        );
      }

      const rawAmount = (swapResponse as { source_amount?: number | string })
        .source_amount;
      if (rawAmount === undefined) {
        throw new Error(
          `Swap ${swapId} has no source_amount — cannot route CCTP fund.`,
        );
      }
      const amount = BigInt(rawAmount);

      const destination = cctpMetaForChainId(42161);
      const maxFee =
        options?.maxFee ??
        computeCctpFastFee(
          await getCachedCctpFee({
            sourceDomain: source.domain,
            destinationDomain: destination.domain,
          }),
          amount,
        );

      const cctp = await this.cctpInbound.fundSwap({
        swapId,
        signer,
        amount,
        maxFee,
        onProgress: options?.onProgress,
        signal: options?.signal,
      });
      return { txHash: cctp.transactionHash ?? cctp.userOpHash, cctp };
    }

    // Direct Permit2 path — Ethereum, Polygon, Arbitrum, or any other
    // chain `signer.chainId` specifies. Unchanged from the original
    // behaviour.

    // 1. Fetch Permit2 funding params
    const funding = await this.getPermit2FundingParamsUnsigned(
      swapId,
      signer.chainId,
    );

    const tokenAddress = funding.sourceTokenAddress;
    const permit2 = PERMIT2_ADDRESS;

    // 2. Check balance — fail early before burning gas on approve/execute
    const balanceCall = encodeBalanceOfCall(tokenAddress, signer.address);
    const balanceResult = await signer.call({
      to: balanceCall.to,
      data: balanceCall.data,
    });
    const balance = decodeUint256(balanceResult);
    if (balance < funding.sourceAmount) {
      throw new Error(
        `Insufficient token balance: have ${balance}, need ${funding.sourceAmount}`,
      );
    }

    // 3. Check allowance to Permit2 — approve if insufficient
    const allowanceCall = encodeAllowanceCall(
      tokenAddress,
      signer.address,
      permit2,
    );
    const allowanceResult = await signer.call({
      to: allowanceCall.to,
      data: allowanceCall.data,
    });
    const allowance = decodeUint256(allowanceResult);

    if (allowance < funding.sourceAmount) {
      // Send approve(Permit2, max)
      const approveData = encodeMaxApproveData(tokenAddress, permit2);
      const approveTxHash = await signer.sendTransaction({
        to: approveData.to,
        data: approveData.data,
        gas: 100_000n,
      });

      const approveReceipt = await signer.waitForReceipt(approveTxHash);
      if (approveReceipt.status !== "success") {
        const reason = await getRevertReason(
          signer,
          approveTxHash,
          approveReceipt.blockNumber,
        );
        throw new Error(`Token approval failed: ${reason}`);
      }
    }

    // 4. Re-fetch fresh funding params (1inch quotes expire quickly)
    const freshFunding = await this.getPermit2FundingParamsUnsigned(
      swapId,
      signer.chainId,
    );

    // 5. Sign the Permit2 EIP-712 typed data
    const signature = await signer.signTypedData(freshFunding.typedData);

    // 6. Encode executeAndCreateWithPermit2 calldata
    const encoded = encodeExecuteAndCreateWithPermit2(
      freshFunding.coordinatorAddress,
      {
        calls: freshFunding.calls,
        preimageHash: freshFunding.preimageHash,
        token: freshFunding.lockTokenAddress,
        claimAddress: freshFunding.claimAddress,
        timelock: freshFunding.timelock,
        depositor: signer.address,
        sourceToken: freshFunding.sourceTokenAddress,
        sourceAmount: freshFunding.sourceAmount,
        nonce: freshFunding.nonce,
        deadline: freshFunding.deadline,
        signature,
      },
    );

    // 7. Simulate before sending to catch reverts without burning gas
    await simulateTransaction(signer, encoded, "Funding transaction");

    // 8. Send the funding transaction
    const txHash = await signer.sendTransaction({
      to: encoded.to,
      data: encoded.data,
      gas: 500_000n,
    });

    // 9. Wait for receipt
    const receipt = await signer.waitForReceipt(txHash);
    if (receipt.status !== "success") {
      const reason = await getRevertReason(signer, txHash, receipt.blockNumber);
      throw new Error(`Funding transaction failed: ${reason}`);
    }

    return { txHash };
  }

  /**
   * Refund an EVM-sourced swap using an external wallet.
   *
   * Fetches refund calldata from the server, sends the transaction via the
   * provided signer, and waits for the receipt.
   *
   * Use this for manual (timelock-based) refunds where the user pays gas.
   * For collaborative (gasless) refunds, use {@link collabRefundEvmWithSigner}.
   *
   * @param swapId - The UUID of the swap
   * @param signer - An {@link EvmSigner} wrapping the user's wallet
   * @param mode - "swap-back" (refund as original token via DEX) or "direct" (refund as WBTC)
   * @returns The refund transaction hash
   */
  async refundEvmWithSigner(
    swapId: string,
    signer: EvmSigner,
    mode: "swap-back" | "direct" = "swap-back",
  ): Promise<{ txHash: string }> {
    const result = await this.refundSwap(swapId, { mode });

    if (!result.evmRefundData) {
      throw new Error(
        `Unable to get EVM refund data for: ${swapId}. ${result.message}`,
      );
    }

    if (!result.evmRefundData.timelockExpired) {
      const expiryDate = new Date(
        result.evmRefundData.timelockExpiry * 1000,
      ).toISOString();
      throw new Error(
        `Refund timelock has not expired yet (expires ${expiryDate}). Use collabRefundEvmWithSigner for instant refunds.`,
      );
    }

    // Simulate before sending to catch reverts without burning gas
    await simulateTransaction(
      signer,
      result.evmRefundData,
      "Refund transaction",
    );

    const txHash = await signer.sendTransaction({
      to: result.evmRefundData.to,
      data: result.evmRefundData.data,
      gas: 500_000n,
    });

    const receipt = await signer.waitForReceipt(txHash);
    if (receipt.status !== "success") {
      const reason = await getRevertReason(signer, txHash, receipt.blockNumber);
      throw new Error(`Refund transaction reverted: ${reason}`);
    }

    return { txHash };
  }

  /**
   * Collaborative refund of an EVM-sourced swap using an external wallet.
   *
   * Signs the EIP-712 `CollabRefund` digest with the provided signer and
   * submits it to the server, which cosigns and executes the refund on-chain.
   * Gasless for the user — no timelock wait required.
   *
   * For gasless (Permit2) swaps where the SDK's embedded key is the depositor,
   * use {@link collabRefundEvmSwap} instead (no external signer needed).
   *
   * @param swapId - The UUID of the swap
   * @param signer - An {@link EvmSigner} wrapping the user's wallet
   * @param settlement - "swap-back" (original token via DEX) or "direct" (WBTC)
   * @returns The refund transaction hash
   */
  async collabRefundEvmWithSigner(
    swapId: string,
    signer: EvmSigner,
    settlement: "swap-back" | "direct" = "direct",
  ): Promise<{ txHash: string }> {
    const { typedData, params } = await this.buildCollabRefundEvmTypedData(
      swapId,
      settlement,
    );

    const signature = await signer.signTypedData(typedData);
    const { v, r, s } = parseSignature(signature);

    const result = await this.submitCollabRefundEvm(swapId, {
      v,
      r,
      s,
      depositor_address: signer.address,
      mode: settlement,
      sweep_token: params.sweepToken,
      min_amount_out: params.minAmountOut,
    });

    return { txHash: result.txHash };
  }

  /**
   * Fund an EVM-sourced swap via the gasless relay.
   *
   * Signs the Permit2 authorization and (optionally) an EIP-2612 permit
   * off-chain, then POSTs them to the server which submits the on-chain
   * transactions on behalf of the user. No wallet or ETH required.
   *
   * @param swapId - The UUID of the swap (must have been created with gasless=true)
   * @returns The relay transaction hash
   *
   * @example
   * ```ts
   * const result = await client.fundSwapGasless(swap.response.id);
   * console.log("Funded via relay:", result.txHash);
   * ```
   */
  async fundSwapGasless(swapId: string): Promise<{ txHash: string }> {
    // 1. Look up stored swap to get the secret key
    const storedSwap = await this.getStoredSwap(swapId);
    if (!storedSwap) {
      throw new Error(
        `Swap ${swapId} not found in local storage. Cannot sign without the secret key.`,
      );
    }

    const swap = await this.getSwap(swapId);

    if (
      swap.direction !== "evm_to_arkade" &&
      swap.direction !== "evm_to_bitcoin" &&
      swap.direction !== "evm_to_lightning"
    ) {
      throw new Error(
        `Expected evm_to_arkade/evm_to_bitcoin/evm_to_lightning swap, got ${swap.direction}. Gasless fund is for EVM-sourced swaps.`,
      );
    }

    const chainId = (swap as { evm_chain_id: number }).evm_chain_id;

    // 2. Fetch Permit2 funding data (includes fee transfer in calls + EIP-2612 data)
    const baseUrl = this.#config.baseUrl.replace(/\/$/, "");
    const url = `${baseUrl}/swap/${swapId}/swap-and-lock-calldata-permit2`;
    const headers: Record<string, string> = {};
    if (this.#config.orgCode) {
      headers["X-Org-Code"] = this.#config.orgCode;
    }

    const resp = await fetch(url, { headers });
    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(
        `Failed to get Permit2 funding data: ${resp.status} ${body}`,
      );
    }

    const serverData = (await resp.json()) as {
      coordinator_address: string;
      permit2_address: string;
      source_token_address: string;
      source_amount: string;
      lock_token_address: string;
      preimage_hash: string;
      claim_address: string;
      timelock: number;
      calls: Array<{ target: string; value: string; call_data: string }>;
      calls_hash: string;
      eip2612?: {
        supported: boolean;
        already_approved: boolean;
        nonce: number;
        domain_separator: string;
      };
      relay_fee?: string;
    };

    // 3. Generate random Permit2 nonce and deadline
    const nonceBytes = new Uint8Array(32);
    crypto.getRandomValues(nonceBytes);
    const nonce = BigInt(
      `0x${Array.from(nonceBytes)
        .map((b) => b.toString(16).padStart(2, "0"))
        .join("")}`,
    );
    const deadline = BigInt(Math.floor(Date.now() / 1000) + 30 * 60); // 30 minutes

    const sourceAmount = BigInt(serverData.source_amount);

    // 4. Build and sign Permit2 EIP-712 digest
    const digest = buildPermit2FundingDigest({
      chainId,
      coordinatorAddress: serverData.coordinator_address,
      sourceToken: serverData.source_token_address,
      sourceAmount,
      preimageHash: serverData.preimage_hash,
      lockToken: serverData.lock_token_address,
      claimAddress: serverData.claim_address,
      refundAddress: serverData.coordinator_address,
      timelock: serverData.timelock,
      callsHash: serverData.calls_hash,
      nonce,
      deadline,
    });
    // 4b. Use the EVM key (deterministic for new swaps, per-swap for legacy)
    const evmKey = this.#getEvmSigningKey();
    const permit2Sig = signEvmDigest(evmKey, digest);
    const rClean = permit2Sig.r.replace(/^0x/, "");
    const sClean = permit2Sig.s.replace(/^0x/, "");
    const vHex = permit2Sig.v.toString(16).padStart(2, "0");
    const compactSignature = `0x${rClean}${sClean}${vHex}`;

    // 5. Derive depositor address from EVM key
    const depositorAddress = deriveEvmAddress(evmKey);

    // 6. If EIP-2612 needed (token supports it and not yet approved to Permit2)
    let eip2612Permit:
      | {
          v: number;
          r: string;
          s: string;
          value: string;
          deadline: number;
          nonce: number;
        }
      | undefined;

    if (serverData.eip2612?.supported && !serverData.eip2612.already_approved) {
      const maxUint256 = BigInt(
        "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
      );
      const eip2612Digest = buildEip2612PermitDigest({
        domainSeparator: serverData.eip2612.domain_separator,
        owner: depositorAddress,
        spender: PERMIT2_ADDRESS,
        value: maxUint256,
        nonce: serverData.eip2612.nonce,
        deadline,
      });
      const sig = signEvmDigest(evmKey, eip2612Digest);
      eip2612Permit = {
        v: sig.v,
        r: sig.r.replace(/^0x/, ""),
        s: sig.s.replace(/^0x/, ""),
        value: maxUint256.toString(),
        deadline: Number(deadline),
        nonce: serverData.eip2612.nonce,
      };
    }

    // 7. POST to fund-gasless relay endpoint
    const fundUrl = `${baseUrl}/swap/${swapId}/fund-gasless`;
    const fundResp = await fetch(fundUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...headers,
      },
      body: JSON.stringify({
        permit2_nonce: nonce.toString(),
        permit2_deadline: Number(deadline),
        permit2_signature: compactSignature,
        calls: serverData.calls,
        eip2612_permit: eip2612Permit,
      }),
    });

    if (!fundResp.ok) {
      const body = await fundResp.text();
      throw new Error(`Gasless fund relay failed: ${fundResp.status} ${body}`);
    }

    const result = (await fundResp.json()) as {
      id: string;
      status: string;
      tx_hash: string;
      message: string;
    };

    return { txHash: result.tx_hash };
  }

  /**
   * Get the ephemeral depositor key for a gasless swap funded swap.
   *
   * Returns the private key and derived EVM address of the ephemeral
   * depositor used in a gasless swap. Use this to build a wallet/signer
   * for recovering stuck funds via {@link recoverGaslessFunds}.
   *
   * The key is deterministically derived from the user's mnemonic and
   * the swap's key index, so it can be re-derived even if local storage
   * is rebuilt from the mnemonic.
   *
   * @param swapId - The UUID of the swap
   * @returns The depositor's private key (hex) and EVM address
   *
   * @example
   * ```ts
   * const { privateKey, address } = await client.getSwapDepositorKey(swapId);
   * // Build a viem wallet from the key:
   * const account = privateKeyToAccount(privateKey);
   * ```
   */
  async getSwapDepositorKey(
    swapId: string,
  ): Promise<{ privateKey: string; address: string }> {
    const storedSwap = await this.getStoredSwap(swapId);
    if (!storedSwap) {
      throw new Error(
        `Swap ${swapId} not found in local storage. Cannot recover without the secret key.`,
      );
    }

    const evmKey = this.#getEvmSigningKey();
    const privateKey = evmKey.startsWith("0x") ? evmKey : `0x${evmKey}`;
    const address = deriveEvmAddress(evmKey);

    return { privateKey, address };
  }

  /**
   * Recover ERC-20 tokens stuck in a gasless swap's ephemeral depositor address.
   *
   * When a gasless swap fails after the user transfers tokens to the
   * SDK-derived depositor address but before the Permit2 tx is published,
   * this method sweeps the tokens back to a destination address.
   *
   * Requires an {@link EvmSigner} backed by the depositor's private key
   * (obtainable via {@link getSwapDepositorKey}). The depositor needs
   * a small amount of ETH/POL for gas — the caller must fund it beforehand.
   *
   * @param swapId - The UUID of the swap whose depositor holds stuck tokens
   * @param depositorSigner - An {@link EvmSigner} backed by the depositor's private key
   * @param destination - EVM address to send recovered tokens to
   * @returns The recovery transaction hash and recovered amount
   *
   * @example
   * ```ts
   * // 1. Get the depositor key
   * const { privateKey } = await client.getSwapDepositorKey(swapId);
   *
   * // 2. Build a signer from the depositor key (e.g. with viem)
   * const depositorAccount = privateKeyToAccount(privateKey);
   * const depositorWallet = createWalletClient({ account: depositorAccount, ... });
   * const depositorSigner = buildEvmSigner(depositorWallet, publicClient);
   *
   * // 3. Fund the depositor with ETH for gas, then recover
   * const result = await client.recoverGaslessFunds(swapId, depositorSigner, myAddress);
   * console.log(`Recovered ${result.amount} tokens, tx: ${result.txHash}`);
   * ```
   */
  async recoverGaslessFunds(
    swapId: string,
    depositorSigner: EvmSigner,
    destination: string,
  ): Promise<{ txHash: string; amount: string }> {
    const storedSwap = await this.getStoredSwap(swapId);
    if (!storedSwap) {
      throw new Error(
        `Swap ${swapId} not found in local storage. Cannot recover without the secret key.`,
      );
    }

    const swap = await this.getSwap(swapId);
    if (
      swap.direction !== "evm_to_arkade" &&
      swap.direction !== "evm_to_bitcoin" &&
      swap.direction !== "evm_to_lightning"
    ) {
      throw new Error(
        `Expected EVM-sourced swap, got ${swap.direction}. Recovery is only for gasless EVM swaps.`,
      );
    }

    const tokenAddress = (swap as { source_token: { token_id: string } })
      .source_token.token_id;

    // Check the depositor's token balance
    const { encodeBalanceOfCall, encodeTransferCall, decodeUint256 } =
      await import("./evm/wallet.js");
    const balanceCall = encodeBalanceOfCall(
      tokenAddress,
      depositorSigner.address,
    );
    const balanceResult = await depositorSigner.call(balanceCall);

    const balance = decodeUint256(balanceResult || "0x0");
    if (balance === 0n) {
      throw new Error(
        `No tokens found at depositor address ${depositorSigner.address}. Nothing to recover.`,
      );
    }

    // Transfer all tokens to destination
    const transferCall = encodeTransferCall(tokenAddress, destination, balance);
    const txHash = await depositorSigner.sendTransaction(transferCall);

    const receipt = await depositorSigner.waitForReceipt(txHash);
    if (receipt.status !== "success") {
      throw new Error(`Recovery transaction reverted: ${txHash}`);
    }

    return { txHash, amount: balance.toString() };
  }

  /**
   * Check if a swap's VTXO has been received on Arkade.
   *
   * For Arkade-destination swaps (EVM/Bitcoin/Lightning → Arkade), queries the
   * Arkade indexer for VTXOs at the `target_arkade_address` and checks if any
   * VTXO's txid matches the swap's `btc_claim_txid`.
   *
   * @param swapId - The UUID of the swap
   * @returns `true` if the VTXO matching `btc_claim_txid` was found
   */
  async hasReceivedVtxo(swapId: string): Promise<boolean> {
    const swap = await this.getSwap(swapId);

    // Extract target_arkade_address and btc_claim_txid based on direction
    let targetArkadeAddress: string | undefined;
    let btcClaimTxid: string | null | undefined;
    let network: string | undefined;

    if (swap.direction === "evm_to_arkade") {
      const s = swap as EvmToArkadeSwapResponse & { direction: string };
      targetArkadeAddress = s.target_arkade_address;
      btcClaimTxid = s.btc_claim_txid;
      network = s.network;
    } else if (swap.direction === "btc_to_arkade") {
      const s = swap as BtcToArkadeSwapResponse & { direction: string };
      targetArkadeAddress = s.target_arkade_address;
      btcClaimTxid = s.arkade_claim_txid;
      network = s.network;
    } else if (swap.direction === "lightning_to_arkade") {
      const s = swap as LightningToArkadeSwapResponse & { direction: string };
      targetArkadeAddress = s.target_arkade_address;
      btcClaimTxid = s.btc_claim_txid;
      network = s.network;
    } else {
      throw new Error(
        `hasReceivedVtxo only works for Arkade-destination swaps, got ${swap.direction}`,
      );
    }

    if (!btcClaimTxid) {
      return false;
    }

    const { ArkAddress, RestIndexerProvider } = await import("@arkade-os/sdk");
    const { hex } = await import("@scure/base");

    const serverUrl = this.#config.arkadeServerUrl;
    if (!serverUrl) {
      throw new Error(`No Arkade server URL for network: ${network}`);
    }

    const decoded = ArkAddress.decode(targetArkadeAddress);
    const pkScript = hex.encode(decoded.pkScript);
    const indexer = new RestIndexerProvider(serverUrl);

    const { vtxos } = await indexer.getVtxos({
      scripts: [pkScript],
    });

    return vtxos.some((v) => v.txid === btcClaimTxid);
  }
}
