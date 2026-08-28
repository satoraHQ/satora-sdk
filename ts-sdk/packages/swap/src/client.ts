/**
 * @satora/swap — the Satora swap client.
 *
 * A standalone `Client` with the same public surface as the legacy
 * `@lendasat/lendaswap-sdk-pure` client, so it is a drop-in replacement. For
 * now every legacy method is forwarded to an internally-owned legacy client
 * instance (see the `@deprecated` delegators below). New Satora-native features
 * are added directly on this class, and
 * individual delegators get replaced with native implementations over time.
 */
import {
  DEFAULT_ELECTRUM_WS_URLS,
  Client as LegacyClient,
  type ClientBuilder as LegacyClientBuilder,
  type SwapStatus,
} from "@lendasat/lendaswap-sdk-pure";
import type { SwapActions } from "./actions/types.js";
import { ArkadeContractManager } from "./contracts/arkade-manager.js";
import { defaultArkadeServerUrl } from "./contracts/arkade-network.js";
import { BitcoinContractManager } from "./contracts/bitcoin-manager.js";
import { DEFAULT_ESPLORA_URLS } from "./contracts/bitcoin-reader-esplora.js";
import { EvmContractManager } from "./contracts/evm-manager.js";
import { defaultEvmReaders } from "./contracts/evm-reader-viem.js";
import type { ContractManager, Ledger } from "./contracts/types.js";
import { HintTracker } from "./hints/hint-tracker.js";
import { SwapWorker } from "./hints/swap-worker.js";
import { WsStatusSource } from "./hints/ws-status-source.js";
import { swapToTracked } from "./tracker/from-swap.js";
import {
  type ActionSubscriber,
  SwapTracker,
  type TrackedSwap,
} from "./tracker/swap-tracker.js";

/**
 * Hint-driven auto-execution, opt-in via {@link ClientBuilder.withAutoClaim}.
 *
 * When set, tracking also opens the server status WebSocket (a faster trigger
 * than the chain poll) and runs a {@link SwapWorker}: it auto-claims a swap the
 * moment the chain confirms it is claimable, and surfaces the rest — a manual
 * `fund`, or a refund the user must confirm — via {@link onActionRequired}.
 */
type AutoClaimConfig = {
  /**
   * Surface an action that needs the user rather than being auto-run — a manual
   * `fund`, or a refund to confirm. The seam a frontend notification center
   * plugs into. Omit to only auto-claim and ignore the rest.
   */
  onActionRequired?: (swapId: string, actions: SwapActions) => void;
};

/** How the client should set up observe-mode tracking. */
type TrackingConfig = {
  /** Whether tracking is enabled at all (default on; `withoutTracking()` clears). */
  enabled: boolean;
  /**
   * The swap server URL (from `withBaseUrl`). When set, tracking opens the
   * server's status WebSocket as a HINT feed: a pushed transition triggers a
   * targeted chain verify, so the chain pollers can stay slow. Chain reads
   * remain the source of truth.
   */
  serverUrl?: string;
  /** Explicit per-ledger managers, bypassing auto-construction (advanced/testing). */
  managers?: Map<Ledger, ContractManager>;
  /** Ark server URL override; defaults to the mainnet server when unset. */
  arkadeServerUrl?: string;
  /** Per-chain EVM RPC overrides; the EVM manager is auto-built either way. */
  evmRpcUrls?: Record<number, string>;
  /** Esplora REST URL override for the Bitcoin manager; defaults to mainnet mempool.space. */
  esploraUrl?: string;
  /**
   * Electrum WebSocket URL (Fulcrum). When set, the Bitcoin manager reads via
   * Electrum with scripthash-subscription push reconciles; esplora stays the
   * fallback on Electrum errors.
   */
  electrumWsUrl?: string;
  /**
   * Network the Bitcoin HTLC addresses live on — needed by the Electrum
   * reader to compute scripthashes. Defaults to mainnet; a signet/regtest
   * deployment must set it or Electrum address decoding fails.
   */
  bitcoinNetwork?: "mainnet" | "testnet" | "signet" | "regtest";
  /**
   * Confirmations a Bitcoin funding tx needs before it observes as
   * `confirmed` (gating e.g. the evm→bitcoin claim). Default `0`: accept
   * 0-conf, trusting the funder not to double-spend. `1` = wait for a block.
   */
  bitcoinMinConfirmations?: number;
  /**
   * Local tick interval (ms) — recomputes actions off extrapolated clocks, no
   * chain reads. Chain reads happen on server hints and, for swaps whose
   * client leg holds funds, on the tracker's at-risk cadence. Defaults to 5s.
   */
  refreshIntervalMs?: number;
  /** Opt-in hint-driven auto-claim; unset leaves tracking observe-only. */
  autoClaim?: AutoClaimConfig;
  /**
   * Verify against the chain instead of trusting server hints. Off by default
   * (TEMP while the chain monitors are buggy): tracking then runs a
   * {@link HintTracker} that derives actions from the server's pushed status,
   * with zero chain access. Setting explicit `managers` implies chain-verified
   * tracking (they exist for nothing else).
   */
  chainVerified?: boolean;
};

export class Client {
  /** The wrapped legacy client. Calls are forwarded here until migrated. */
  readonly #legacy: LegacyClient;
  /** Tracking configuration; managers are built lazily from it on first use. */
  readonly #tracking: TrackingConfig;
  /** Per-ledger monitors, resolved once from {@link #tracking}. */
  #managers: Map<Ledger, ContractManager> | undefined;
  #tracker: SwapTracker | HintTracker | undefined;
  /** Hint-driven auto-claim worker; only built when `withAutoClaim` opted in. */
  #worker: SwapWorker | undefined;
  #startPromise?: Promise<void>;
  /** Swap ids whose settled status was already re-fetched this session. */
  readonly #settledSyncDone = new Set<string>();

  /**
   * Creates a new Client instance.
   *
   * Prefer {@link Client.builder} for new code.
   */
  constructor(...args: ConstructorParameters<typeof LegacyClient>);
  /** @internal Wrap an already-built legacy client, optionally with tracking. */
  constructor(legacy: LegacyClient, tracking?: TrackingConfig);
  constructor(
    ...args:
      | ConstructorParameters<typeof LegacyClient>
      | [legacy: LegacyClient, tracking?: TrackingConfig]
  ) {
    if (args[0] instanceof LegacyClient) {
      this.#legacy = args[0];
      this.#tracking = (args[1] as TrackingConfig | undefined) ?? {
        enabled: false,
      };
    } else {
      this.#legacy = new LegacyClient(
        ...(args as ConstructorParameters<typeof LegacyClient>),
      );
      // A raw (non-builder) client has no tracking config to build from.
      this.#tracking = { enabled: false };
    }
  }

  /** Start building a {@link Client}. */
  static builder(): ClientBuilder {
    return new ClientBuilder();
  }

  /** Escape hatch: the underlying legacy client, during migration. */
  get legacy(): LegacyClient {
    return this.#legacy;
  }

  // --- delegated legacy surface (getters) ---

  /** Delegated to the legacy client (migration checkpoint). */
  get api(): LegacyClient["api"] {
    return this.#legacy.api;
  }

  /** Delegated to the legacy client (migration checkpoint). */
  get baseUrl(): LegacyClient["baseUrl"] {
    return this.#legacy.baseUrl;
  }

  /** Delegated to the legacy client (migration checkpoint). */
  get cctpInbound(): LegacyClient["cctpInbound"] {
    return this.#legacy.cctpInbound;
  }

  /** Delegated to the legacy client (migration checkpoint). */
  get swapStorage(): LegacyClient["swapStorage"] {
    return this.#legacy.swapStorage;
  }

  // --- delegated legacy surface (methods) ---

  /** Delegated to the legacy client (migration checkpoint). */
  subscribeToSwaps(
    ...args: Parameters<LegacyClient["subscribeToSwaps"]>
  ): ReturnType<LegacyClient["subscribeToSwaps"]> {
    return this.#legacy.subscribeToSwaps(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  unsubscribeFromSwaps(
    ...args: Parameters<LegacyClient["unsubscribeFromSwaps"]>
  ): ReturnType<LegacyClient["unsubscribeFromSwaps"]> {
    return this.#legacy.unsubscribeFromSwaps(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  closeSwapStatusSocket(
    ...args: Parameters<LegacyClient["closeSwapStatusSocket"]>
  ): ReturnType<LegacyClient["closeSwapStatusSocket"]> {
    return this.#legacy.closeSwapStatusSocket(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getMnemonic(
    ...args: Parameters<LegacyClient["getMnemonic"]>
  ): ReturnType<LegacyClient["getMnemonic"]> {
    return this.#legacy.getMnemonic(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  loadMnemonic(
    ...args: Parameters<LegacyClient["loadMnemonic"]>
  ): ReturnType<LegacyClient["loadMnemonic"]> {
    return this.#legacy.loadMnemonic(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getNostrKeyHex(
    ...args: Parameters<LegacyClient["getNostrKeyHex"]>
  ): ReturnType<LegacyClient["getNostrKeyHex"]> {
    return this.#legacy.getNostrKeyHex(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getUserIdXpub(
    ...args: Parameters<LegacyClient["getUserIdXpub"]>
  ): ReturnType<LegacyClient["getUserIdXpub"]> {
    return this.#legacy.getUserIdXpub(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  deriveSwapParams(
    ...args: Parameters<LegacyClient["deriveSwapParams"]>
  ): ReturnType<LegacyClient["deriveSwapParams"]> {
    return this.#legacy.deriveSwapParams(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  deriveSwapParamsAtIndex(
    ...args: Parameters<LegacyClient["deriveSwapParamsAtIndex"]>
  ): ReturnType<LegacyClient["deriveSwapParamsAtIndex"]> {
    return this.#legacy.deriveSwapParamsAtIndex(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getEvmAddress(
    ...args: Parameters<LegacyClient["getEvmAddress"]>
  ): ReturnType<LegacyClient["getEvmAddress"]> {
    return this.#legacy.getEvmAddress(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getKeyIndex(
    ...args: Parameters<LegacyClient["getKeyIndex"]>
  ): ReturnType<LegacyClient["getKeyIndex"]> {
    return this.#legacy.getKeyIndex(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  setKeyIndex(
    ...args: Parameters<LegacyClient["setKeyIndex"]>
  ): ReturnType<LegacyClient["setKeyIndex"]> {
    return this.#legacy.setKeyIndex(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getStatus(
    ...args: Parameters<LegacyClient["getStatus"]>
  ): ReturnType<LegacyClient["getStatus"]> {
    return this.#legacy.getStatus(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  healthCheck(
    ...args: Parameters<LegacyClient["healthCheck"]>
  ): ReturnType<LegacyClient["healthCheck"]> {
    return this.#legacy.healthCheck(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getVersion(
    ...args: Parameters<LegacyClient["getVersion"]>
  ): ReturnType<LegacyClient["getVersion"]> {
    return this.#legacy.getVersion(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getSupportAgents(
    ...args: Parameters<LegacyClient["getSupportAgents"]>
  ): ReturnType<LegacyClient["getSupportAgents"]> {
    return this.#legacy.getSupportAgents(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getMtp(
    ...args: Parameters<LegacyClient["getMtp"]>
  ): ReturnType<LegacyClient["getMtp"]> {
    return this.#legacy.getMtp(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getTokens(
    ...args: Parameters<LegacyClient["getTokens"]>
  ): ReturnType<LegacyClient["getTokens"]> {
    return this.#legacy.getTokens(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getSwapPairs(
    ...args: Parameters<LegacyClient["getSwapPairs"]>
  ): ReturnType<LegacyClient["getSwapPairs"]> {
    return this.#legacy.getSwapPairs(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getChainConfig(
    ...args: Parameters<LegacyClient["getChainConfig"]>
  ): ReturnType<LegacyClient["getChainConfig"]> {
    return this.#legacy.getChainConfig(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getReferralFee(
    ...args: Parameters<LegacyClient["getReferralFee"]>
  ): ReturnType<LegacyClient["getReferralFee"]> {
    return this.#legacy.getReferralFee(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getNetworkFees(
    ...args: Parameters<LegacyClient["getNetworkFees"]>
  ): ReturnType<LegacyClient["getNetworkFees"]> {
    return this.#legacy.getNetworkFees(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getDexQuote(
    ...args: Parameters<LegacyClient["getDexQuote"]>
  ): ReturnType<LegacyClient["getDexQuote"]> {
    return this.#legacy.getDexQuote(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  composeQuote(
    ...args: Parameters<LegacyClient["composeQuote"]>
  ): ReturnType<LegacyClient["composeQuote"]> {
    return this.#legacy.composeQuote(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getQuote(
    ...args: Parameters<LegacyClient["getQuote"]>
  ): ReturnType<LegacyClient["getQuote"]> {
    return this.#legacy.getQuote(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createLightningToEvmSwap(
    ...args: Parameters<LegacyClient["createLightningToEvmSwap"]>
  ): ReturnType<LegacyClient["createLightningToEvmSwap"]> {
    return this.#legacy.createLightningToEvmSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getLightningSendQuote(
    ...args: Parameters<LegacyClient["getLightningSendQuote"]>
  ): ReturnType<LegacyClient["getLightningSendQuote"]> {
    return this.#legacy.getLightningSendQuote(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getSwap(
    ...args: Parameters<LegacyClient["getSwap"]>
  ): ReturnType<LegacyClient["getSwap"]> {
    return this.#legacy.getSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getBulkStatus(
    ...args: Parameters<LegacyClient["getBulkStatus"]>
  ): ReturnType<LegacyClient["getBulkStatus"]> {
    return this.#legacy.getBulkStatus(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getStoredSwap(
    ...args: Parameters<LegacyClient["getStoredSwap"]>
  ): ReturnType<LegacyClient["getStoredSwap"]> {
    return this.#legacy.getStoredSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  listAllSwaps(
    ...args: Parameters<LegacyClient["listAllSwaps"]>
  ): ReturnType<LegacyClient["listAllSwaps"]> {
    return this.#legacy.listAllSwaps(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  deleteSwap(
    ...args: Parameters<LegacyClient["deleteSwap"]>
  ): ReturnType<LegacyClient["deleteSwap"]> {
    return this.#legacy.deleteSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  clearSwapStorage(
    ...args: Parameters<LegacyClient["clearSwapStorage"]>
  ): ReturnType<LegacyClient["clearSwapStorage"]> {
    return this.#legacy.clearSwapStorage(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  recoverSwaps(
    ...args: Parameters<LegacyClient["recoverSwaps"]>
  ): ReturnType<LegacyClient["recoverSwaps"]> {
    return this.#legacy.recoverSwaps(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  recoverAllSwaps(
    ...args: Parameters<LegacyClient["recoverAllSwaps"]>
  ): ReturnType<LegacyClient["recoverAllSwaps"]> {
    return this.#legacy.recoverAllSwaps(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  amountsForSwap(
    ...args: Parameters<LegacyClient["amountsForSwap"]>
  ): ReturnType<LegacyClient["amountsForSwap"]> {
    return this.#legacy.amountsForSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  claim(
    ...args: Parameters<LegacyClient["claim"]>
  ): ReturnType<LegacyClient["claim"]> {
    return this.#legacy.claim(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  claimViaGasless(
    ...args: Parameters<LegacyClient["claimViaGasless"]>
  ): ReturnType<LegacyClient["claimViaGasless"]> {
    return this.#legacy.claimViaGasless(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  claimArkade(
    ...args: Parameters<LegacyClient["claimArkade"]>
  ): ReturnType<LegacyClient["claimArkade"]> {
    return this.#legacy.claimArkade(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  continueArkadeClaimSwap(
    ...args: Parameters<LegacyClient["continueArkadeClaimSwap"]>
  ): ReturnType<LegacyClient["continueArkadeClaimSwap"]> {
    return this.#legacy.continueArkadeClaimSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  refundSwap(
    ...args: Parameters<LegacyClient["refundSwap"]>
  ): ReturnType<LegacyClient["refundSwap"]> {
    return this.#legacy.refundSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getCollabRefundEvmParams(
    ...args: Parameters<LegacyClient["getCollabRefundEvmParams"]>
  ): ReturnType<LegacyClient["getCollabRefundEvmParams"]> {
    return this.#legacy.getCollabRefundEvmParams(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  buildCollabRefundEvmTypedData(
    ...args: Parameters<LegacyClient["buildCollabRefundEvmTypedData"]>
  ): ReturnType<LegacyClient["buildCollabRefundEvmTypedData"]> {
    return this.#legacy.buildCollabRefundEvmTypedData(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  collabRefundEvmSwap(
    ...args: Parameters<LegacyClient["collabRefundEvmSwap"]>
  ): ReturnType<LegacyClient["collabRefundEvmSwap"]> {
    return this.#legacy.collabRefundEvmSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  submitCollabRefundEvm(
    ...args: Parameters<LegacyClient["submitCollabRefundEvm"]>
  ): ReturnType<LegacyClient["submitCollabRefundEvm"]> {
    return this.#legacy.submitCollabRefundEvm(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createSwap(
    ...args: Parameters<LegacyClient["createSwap"]>
  ): ReturnType<LegacyClient["createSwap"]> {
    return this.#trackAfterCreate(this.#legacy.createSwap(...args));
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createArkadeToEvmSwapGeneric(
    ...args: Parameters<LegacyClient["createArkadeToEvmSwapGeneric"]>
  ): ReturnType<LegacyClient["createArkadeToEvmSwapGeneric"]> {
    return this.#trackAfterCreate(
      this.#legacy.createArkadeToEvmSwapGeneric(...args),
    );
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createBitcoinToEvmSwap(
    ...args: Parameters<LegacyClient["createBitcoinToEvmSwap"]>
  ): ReturnType<LegacyClient["createBitcoinToEvmSwap"]> {
    return this.#trackAfterCreate(this.#legacy.createBitcoinToEvmSwap(...args));
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createBitcoinToArkadeSwap(
    ...args: Parameters<LegacyClient["createBitcoinToArkadeSwap"]>
  ): ReturnType<LegacyClient["createBitcoinToArkadeSwap"]> {
    return this.#trackAfterCreate(
      this.#legacy.createBitcoinToArkadeSwap(...args),
    );
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createLightningToArkadeSwap(
    ...args: Parameters<LegacyClient["createLightningToArkadeSwap"]>
  ): ReturnType<LegacyClient["createLightningToArkadeSwap"]> {
    return this.#trackAfterCreate(
      this.#legacy.createLightningToArkadeSwap(...args),
    );
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createArkadeToLightningSwap(
    ...args: Parameters<LegacyClient["createArkadeToLightningSwap"]>
  ): ReturnType<LegacyClient["createArkadeToLightningSwap"]> {
    return this.#trackAfterCreate(
      this.#legacy.createArkadeToLightningSwap(...args),
    );
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createEvmToLightningSwap(
    ...args: Parameters<LegacyClient["createEvmToLightningSwap"]>
  ): ReturnType<LegacyClient["createEvmToLightningSwap"]> {
    return this.#trackAfterCreate(
      this.#legacy.createEvmToLightningSwap(...args),
    );
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createEvmToArkadeSwapGeneric(
    ...args: Parameters<LegacyClient["createEvmToArkadeSwapGeneric"]>
  ): ReturnType<LegacyClient["createEvmToArkadeSwapGeneric"]> {
    return this.#trackAfterCreate(
      this.#legacy.createEvmToArkadeSwapGeneric(...args),
    );
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createEvmToBitcoinSwap(
    ...args: Parameters<LegacyClient["createEvmToBitcoinSwap"]>
  ): ReturnType<LegacyClient["createEvmToBitcoinSwap"]> {
    return this.#trackAfterCreate(this.#legacy.createEvmToBitcoinSwap(...args));
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getCoordinatorFundingCallDataPermit2(
    ...args: Parameters<LegacyClient["getCoordinatorFundingCallDataPermit2"]>
  ): ReturnType<LegacyClient["getCoordinatorFundingCallDataPermit2"]> {
    return this.#legacy.getCoordinatorFundingCallDataPermit2(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getPermit2FundingParamsUnsigned(
    ...args: Parameters<LegacyClient["getPermit2FundingParamsUnsigned"]>
  ): ReturnType<LegacyClient["getPermit2FundingParamsUnsigned"]> {
    return this.#legacy.getPermit2FundingParamsUnsigned(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  fundSwap(
    ...args: Parameters<LegacyClient["fundSwap"]>
  ): ReturnType<LegacyClient["fundSwap"]> {
    return this.#legacy.fundSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  refundEvmWithSigner(
    ...args: Parameters<LegacyClient["refundEvmWithSigner"]>
  ): ReturnType<LegacyClient["refundEvmWithSigner"]> {
    return this.#legacy.refundEvmWithSigner(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  collabRefundEvmWithSigner(
    ...args: Parameters<LegacyClient["collabRefundEvmWithSigner"]>
  ): ReturnType<LegacyClient["collabRefundEvmWithSigner"]> {
    return this.#legacy.collabRefundEvmWithSigner(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  fundSwapGasless(
    ...args: Parameters<LegacyClient["fundSwapGasless"]>
  ): ReturnType<LegacyClient["fundSwapGasless"]> {
    return this.#legacy.fundSwapGasless(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getSwapDepositorKey(
    ...args: Parameters<LegacyClient["getSwapDepositorKey"]>
  ): ReturnType<LegacyClient["getSwapDepositorKey"]> {
    return this.#legacy.getSwapDepositorKey(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getEvmDepositorKey(
    ...args: Parameters<LegacyClient["getEvmDepositorKey"]>
  ): ReturnType<LegacyClient["getEvmDepositorKey"]> {
    return this.#legacy.getEvmDepositorKey(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getRefundedEvmSwapContinuation(
    ...args: Parameters<LegacyClient["getRefundedEvmSwapContinuation"]>
  ): ReturnType<LegacyClient["getRefundedEvmSwapContinuation"]> {
    return this.#legacy.getRefundedEvmSwapContinuation(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  continueRefundedEvmSwap(
    ...args: Parameters<LegacyClient["continueRefundedEvmSwap"]>
  ): ReturnType<LegacyClient["continueRefundedEvmSwap"]> {
    return this.#trackAfterRetry(this.#legacy.continueRefundedEvmSwap(...args));
  }

  /** Delegated to the legacy client (migration checkpoint). */
  recoverGaslessFunds(
    ...args: Parameters<LegacyClient["recoverGaslessFunds"]>
  ): ReturnType<LegacyClient["recoverGaslessFunds"]> {
    return this.#legacy.recoverGaslessFunds(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  hasReceivedVtxo(
    ...args: Parameters<LegacyClient["hasReceivedVtxo"]>
  ): ReturnType<LegacyClient["hasReceivedVtxo"]> {
    return this.#legacy.hasReceivedVtxo(...args);
  }

  // --- Satora-native features go below ---

  /**
   * Start observing the user's active swaps and deriving each one's next action.
   *
   * Loads stored swaps, maps them to {@link TrackedSwap}s, and derives each
   * one's actions — by default from the server's status hints alone (TEMP while
   * the chain monitors are buggy; see
   * {@link ClientBuilder.withChainVerifiedTracking} to re-enable the per-ledger
   * chain monitors). Idempotent. Requires
   * the client to have been built with {@link ClientBuilder.withTracking}.
   * Subscribe with {@link subscribeToActions}; release with
   * {@link stopTracking}.
   */
  startTracking(): Promise<void> {
    if (!this.#tracking.enabled)
      return Promise.reject(
        new Error(
          "tracking is disabled — remove .withoutTracking() to enable it",
        ),
      );
    // Cache the in-flight promise, not a boolean: a concurrent caller must
    // await the first call's completion, or it would be told "started" while
    // `#tracker` is still unset and crash in `subscribeToActions`.
    this.#startPromise ??= this.#doStartTracking().catch((error) => {
      this.#startPromise = undefined; // let a later call retry cleanly
      throw error;
    });
    return this.#startPromise;
  }

  async #doStartTracking(): Promise<void> {
    try {
      // Explicit managers imply chain verification — they exist for nothing
      // else, and every pre-existing withContractManagers caller expects it.
      const chainVerified =
        this.#tracking.chainVerified || this.#tracking.managers !== undefined;
      const tracker = chainVerified
        ? new SwapTracker(await this.#ensureManagers(), {
            refreshIntervalMs: this.#tracking.refreshIntervalMs ?? 5_000,
          })
        : new HintTracker({
            fetchStatus: async (swapId) =>
              (await this.getSwap(swapId, { updateStorage: true })).status,
            refreshIntervalMs: this.#tracking.refreshIntervalMs ?? 5_000,
          });
      this.#tracker = tracker;
      await tracker.startTracking(await this.#loadTrackedSwaps());
      // Keep STORED statuses converging on chain truth: without this, a swap
      // that settled while the app was closed keeps a stale active status and
      // #loadTrackedSwaps re-tracks it on every future start. Dies with the
      // tracker (stop() drops its subscribers), so no unsub bookkeeping.
      tracker.subscribeToActions((swapId, actions) =>
        this.#syncSettledStatus(swapId, actions),
      );
      this.#startWorker(tracker);
    } catch (error) {
      // A partway failure (e.g. a ledger register/refresh erroring on an RPC or
      // indexer hiccup) left `#tracker`/`#worker` set and some legs registered.
      // Tear them down so `subscribeToActions` still reports "not started" and a
      // retry begins clean, instead of leaking registrations/listeners/sockets.
      this.#worker?.stop();
      this.#worker = undefined;
      this.#tracker?.stop();
      this.#tracker = undefined;
      throw error;
    }
  }

  /**
   * The chain says this swap is finished (`none`) — if the STORED status still
   * reads active, re-fetch and persist it so the next session's
   * {@link #loadTrackedSwaps} settled-filter catches it (instead of tracking
   * and scanning the swap once per session forever). Fire-and-forget with a
   * per-session dedupe: a failed fetch just means one more one-scan round next
   * session, which is exactly the current behavior. A swap whose stored status
   * is already settled costs nothing (no server call).
   */
  #syncSettledStatus(swapId: string, actions: SwapActions): void {
    if (actions.recommended !== "none") return;
    if (this.#settledSyncDone.has(swapId)) return;
    this.#settledSyncDone.add(swapId);
    void (async () => {
      const stored = (await this.listAllSwaps()).find(
        (s) => s.response.id === swapId,
      );
      if (!stored || SETTLED_STORED_STATUSES.has(stored.response.status))
        return;
      await this.getSwap(swapId, { updateStorage: true });
    })().catch((error) => {
      console.warn(
        `Client: refreshing settled status of swap ${swapId} failed:`,
        error,
      );
    });
  }

  /** Every stored swap whose ledgers are observable, mapped to a {@link TrackedSwap}. */
  async #loadTrackedSwaps(): Promise<TrackedSwap[]> {
    const swaps = await this.listAllSwaps();
    return swaps
      .filter((s) => !SETTLED_STORED_STATUSES.has(s.response.status))
      .map((s): TrackedSwap | undefined => {
        // Isolate per swap: one unmappable stored swap (e.g. a legacy record
        // with an unsupported script version) must not prevent every other
        // swap from being tracked and auto-claimed.
        try {
          const tracked = swapToTracked(s);
          if (!tracked) return undefined;
          // Seed hints-only tracking with the last stored status (a possibly
          // stale first derivation; the WS snapshot corrects it on subscribe).
          return { ...tracked, storedStatus: s.response.status };
        } catch (error) {
          console.warn(
            `Client: skipping untrackable stored swap ${s.response.id}:`,
            error,
          );
          return undefined;
        }
      })
      .filter((s): s is TrackedSwap => s !== undefined);
  }

  /**
   * Run a create, then fold the new swap into tracking.
   *
   * When tracking is active, first REJECT a swap this client can't reach: a leg on
   * an unconfigured chain can be neither observed nor claimed, so creating it would
   * strand funds. Throwing here (before the caller funds it) is deliberately
   * stricter than {@link startTracking}, which only *skips* such a pre-existing
   * swap. Otherwise fold the new swap(s) in — fire-and-forget, so a tracking hiccup
   * never fails or delays the create; a no-op when tracking isn't running (a later
   * {@link startTracking} picks it up from storage).
   */
  async #trackAfterCreate<T>(op: Promise<T>): Promise<T> {
    const result = await op;
    const tracker = this.#tracker;
    if (!tracker) return result;

    let swaps: TrackedSwap[];
    try {
      swaps = await this.#loadTrackedSwaps();
    } catch (error) {
      // A storage-read hiccup must not fail a create that already succeeded; the
      // next startTracking reconciles from storage anyway.
      console.warn("Client: loading swaps after create failed:", error);
      return result;
    }

    const created = swaps.find((s) => s.swapId === createdSwapId(result));
    if (created && !tracker.canObserve(created))
      throw new Error(
        `Refusing swap ${created.swapId}: a leg is on a chain this client can't reach, ` +
          `so it can be neither tracked nor claimed. Configure the chain ` +
          `(e.g. ClientBuilder.withEvmRpcUrls) before creating it.`,
      );
    void this.#trackAll(tracker, swaps);
    return result;
  }

  /**
   * Run a retry, then fold its replacement swap into tracking.
   *
   * Unlike a create there is no unreachable-leg
   * rejection: by the time the retry returns, the old VHTLC has already been
   * collab-refunded into the new one, so refusing here can't protect any funds
   * — and {@link SwapTracker.track} skips a swap it can't observe anyway.
   */
  async #trackAfterRetry<T>(op: Promise<T>): Promise<T> {
    const result = await op;
    const tracker = this.#tracker;
    if (!tracker) return result;

    try {
      void this.#trackAll(tracker, await this.#loadTrackedSwaps());
    } catch (error) {
      // Same rationale as #trackAfterCreate: the retry already succeeded.
      console.warn("Client: loading swaps after retry failed:", error);
    }
    return result;
  }

  /** Track each given swap the tracker isn't already watching (isolated per swap). */
  async #trackAll(
    tracker: SwapTracker | HintTracker,
    swaps: TrackedSwap[],
  ): Promise<void> {
    // Isolate per swap: one that fails to register (and rolls itself back) must
    // not stop the rest of the batch from being tracked.
    for (const swap of swaps) {
      try {
        await tracker.track(swap);
      } catch (error) {
        console.warn(`Client: tracking swap ${swap.swapId} failed:`, error);
      }
    }
  }

  /**
   * Wire the server status WebSocket into a {@link SwapWorker} whenever a server
   * URL is configured: pushed status transitions trigger a targeted chain
   * re-verify (`applyHint`), which is what lets the chain pollers stay slow.
   * Auto-EXECUTION on top of that is opt-in: only with `withAutoClaim` does the
   * worker get an `execute` (claims) and an `onActionRequired` surface —
   * otherwise it runs observe-only, hints in, no spends.
   */
  #startWorker(tracker: SwapTracker | HintTracker): void {
    // Fall back to the URL the legacy REST client resolved to (its mainnet
    // default when withBaseUrl was never called): tracking must open the hint
    // feed against the same server the REST calls go to — without this, the
    // default-URL builder flow gets a HintTracker but no hints, freezing
    // tracking at each swap's stored status. A legacy-like stand-in whose
    // `baseUrl` getter throws (no internal config) simply yields no fallback,
    // keeping the old no-worker behavior.
    const serverUrl = this.#tracking.serverUrl ?? legacyBaseUrl(this.#legacy);
    if (!serverUrl) return;
    const autoClaim = this.#tracking.autoClaim;

    const hintSource = new WsStatusSource({ serverUrl });
    const worker = new SwapWorker({
      tracker,
      hintSource,
      execute: autoClaim
        ? async (swapId, actionId) => {
            // The worker only ever auto-runs claims (its AUTO_EXECUTABLE set).
            if (actionId !== "claim")
              throw new Error(`refusing to auto-run action '${actionId}'`);
            const result = await this.claim(swapId);
            if (!result.success)
              throw new Error(result.message ?? "claim failed");
          }
        : undefined,
      onActionRequired: autoClaim?.onActionRequired,
    });
    this.#worker = worker;
    worker.start();
  }

  /**
   * Subscribe to next-action updates: `cb` fires with the current action for each
   * tracked swap immediately, then on every change. Call after
   * {@link startTracking}. Returns an unsubscribe fn.
   */
  subscribeToActions(cb: ActionSubscriber): () => void {
    if (!this.#tracker)
      throw new Error("call startTracking() before subscribeToActions()");
    return this.#tracker.subscribeToActions(cb);
  }

  /** Stop tracking and drop subscribers. The managers themselves are not disposed. */
  stopTracking(): void {
    this.#worker?.stop();
    this.#worker = undefined;
    this.#tracker?.stop();
    this.#tracker = undefined;
    this.#startPromise = undefined;
  }

  /**
   * Resolve the per-ledger managers once — the explicit override if given, else
   * auto-built from config: an {@link ArkadeContractManager} from the configured
   * Ark server URL (defaulting to the mainnet server), clocked by `getMtp`, and an
   * {@link EvmContractManager} from the tested default RPCs.
   */
  async #ensureManagers(): Promise<Map<Ledger, ContractManager>> {
    if (this.#managers) return this.#managers;
    if (this.#tracking.managers) {
      this.#managers = this.#tracking.managers;
      return this.#managers;
    }
    const managers = new Map<Ledger, ContractManager>();
    const {
      arkadeServerUrl,
      evmRpcUrls,
      esploraUrl,
      electrumWsUrl,
      bitcoinNetwork,
      bitcoinMinConfirmations,
    } = this.#tracking;
    // Arkade + Bitcoin share the Bitcoin MTP clock.
    const chainTime = async () => (await this.getMtp()).mtp * 1000;

    // Default to mainnet; dev/other networks override via withArkadeServerUrl.
    const arkadeUrl = arkadeServerUrl ?? defaultArkadeServerUrl("bitcoin");
    if (arkadeUrl) {
      managers.set(
        "arkade",
        await ArkadeContractManager.create({ serverUrl: arkadeUrl, chainTime }),
      );
    }
    // EVM tracks out of the box via tested default RPCs; overrides take priority.
    const readers = defaultEvmReaders(evmRpcUrls);
    if (readers.size > 0)
      managers.set("evm", EvmContractManager.fromDeps({ readers }));
    // Bitcoin observes on-chain HTLCs via esplora. Default to the public pair
    // (mempool.space + blockstream.info) with rotation/failover; an explicit URL
    // replaces them (a dev/regtest node must not fail over to mainnet).
    // Electrum defaults per network (Satora's Fulcrum on mainnet, nothing
    // elsewhere); an explicit URL always wins. Esplora stays the fallback,
    // so the default endpoint being down degrades to plain Esplora reads.
    const network = bitcoinNetwork ?? "mainnet";
    managers.set(
      "bitcoin",
      await BitcoinContractManager.create({
        esploraUrl: esploraUrl ? [esploraUrl] : DEFAULT_ESPLORA_URLS,
        electrumWsUrl: electrumWsUrl ?? DEFAULT_ELECTRUM_WS_URLS[network],
        network,
        chainTime,
        minConfirmations: bitcoinMinConfirmations,
      }),
    );
    this.#managers = managers;
    return managers;
  }
}

/**
 * Builds a {@link Client}. Mirrors the legacy `ClientBuilder` fluent surface and
 * constructs the legacy client internally, then wraps it.
 */
export class ClientBuilder {
  readonly #inner: LegacyClientBuilder = LegacyClient.builder();
  // Observe-mode tracking is on by default; the built client auto-builds its
  // managers from config unless disabled or given an explicit override.
  #trackingEnabled = true;
  #serverUrl: string | undefined;
  #arkadeServerUrl: string | undefined;
  #esploraUrl: string | undefined;
  #electrumWsUrl: string | undefined;
  #bitcoinNetwork: "mainnet" | "testnet" | "signet" | "regtest" | undefined;
  #bitcoinMinConfirmations: number | undefined;
  #evmRpcUrls: Record<number, string> | undefined;
  #managers: Map<Ledger, ContractManager> | undefined;
  #autoClaim: AutoClaimConfig | undefined;
  #chainVerified = false;

  /**
   * Override the EVM JSON-RPC endpoint per chainId. Optional — tracking uses
   * tested public defaults otherwise; an override is tried first, with the
   * defaults kept as fallbacks.
   */
  withEvmRpcUrls(urls: Record<number, string>): this {
    this.#evmRpcUrls = urls;
    return this;
  }

  /**
   * Opt in to hint-driven auto-claim. Tracking then also subscribes to the
   * server status WebSocket (a faster trigger than the chain poll) and, when the
   * chain confirms a swap is claimable, claims it automatically. Actions that
   * need the user — a manual `fund`, or a refund to confirm — are surfaced via
   * `onActionRequired` instead of being run. Off by default: this auto-spends on
   * the user's behalf, so it must be explicit.
   */
  withAutoClaim(options?: {
    onActionRequired?: (swapId: string, actions: SwapActions) => void;
  }): this {
    this.#autoClaim = { onActionRequired: options?.onActionRequired };
    return this;
  }

  /** Turn observe-mode tracking off. */
  withoutTracking(): this {
    this.#trackingEnabled = false;
    return this;
  }

  /**
   * Verify swap state against the chain (per-ledger contract managers) instead
   * of trusting the server's status hints. TEMPORARILY off by default while the
   * chain monitors are being fixed — without it, tracking derives actions from
   * the server's pushed status alone, with zero chain access.
   */
  withChainVerifiedTracking(): this {
    this.#chainVerified = true;
    return this;
  }

  /**
   * Advanced: supply per-ledger {@link ContractManager}s explicitly instead of
   * letting the client auto-build them from config (useful for tests or custom
   * chain sources).
   */
  withContractManagers(managers: Map<Ledger, ContractManager>): this {
    this.#managers = managers;
    return this;
  }

  withBaseUrl(...args: Parameters<LegacyClientBuilder["withBaseUrl"]>): this {
    this.#inner.withBaseUrl(...args);
    this.#serverUrl = args[0];
    return this;
  }

  withReferralCode(
    ...args: Parameters<LegacyClientBuilder["withReferralCode"]>
  ): this {
    this.#inner.withReferralCode(...args);
    return this;
  }

  withOrgCode(...args: Parameters<LegacyClientBuilder["withOrgCode"]>): this {
    this.#inner.withOrgCode(...args);
    return this;
  }

  withDefaultHeaders(
    ...args: Parameters<LegacyClientBuilder["withDefaultHeaders"]>
  ): this {
    this.#inner.withDefaultHeaders(...args);
    return this;
  }

  /**
   * Override the Esplora REST URL. Optional for tracking — the Bitcoin manager
   * otherwise defaults to mainnet mempool.space; set this for a dev/regtest node.
   */
  withEsploraUrl(url: string): this {
    this.#inner.withEsploraUrl(url);
    this.#esploraUrl = url;
    return this;
  }

  /**
   * Set an Electrum server WebSocket URL (a Fulcrum `ws`/`wss` port) for
   * Bitcoin claim/refund chain access. When set, the underlying client
   * prefers it over Esplora for lookups and broadcasts, and waits for HTLC
   * fundings via `blockchain.scripthash.subscribe` push instead of polling.
   * Esplora remains the fallback whenever the Electrum server errors.
   */
  withElectrumWsUrl(url: string): this {
    this.#inner.withElectrumWsUrl(url);
    this.#electrumWsUrl = url;
    return this;
  }

  /**
   * Declare the Bitcoin network of this deployment's HTLC addresses. Only
   * needed together with {@link withElectrumWsUrl} on a non-mainnet
   * deployment — the Electrum reader decodes addresses to scripthashes and
   * defaults to mainnet parameters.
   */
  withBitcoinNetwork(
    network: "mainnet" | "testnet" | "signet" | "regtest",
  ): this {
    this.#bitcoinNetwork = network;
    return this;
  }

  /**
   * Confirmations a Bitcoin funding tx needs before it observes as `confirmed`
   * (gating e.g. the evm→bitcoin claim). Default `0`: claim as soon as the
   * funding hits the mempool, which TRUSTS the funder not to double-spend it
   * (claiming publishes the preimage, so a funder that then replaced its
   * funding could take both legs). Set `1` (or more) for a block-depth policy
   * that doesn't rely on the funder's good behaviour.
   */
  withBitcoinMinConfirmations(minConfirmations: number): this {
    this.#bitcoinMinConfirmations = minConfirmations;
    return this;
  }

  /**
   * Override the Ark server URL. Optional for tracking — it otherwise defaults to
   * the mainnet server; set this to track a non-mainnet (dev/signet) deployment.
   */
  withArkadeServerUrl(url: string): this {
    this.#inner.withArkadeServerUrl(url);
    this.#arkadeServerUrl = url;
    return this;
  }

  withSignerStorage(
    ...args: Parameters<LegacyClientBuilder["withSignerStorage"]>
  ): this {
    this.#inner.withSignerStorage(...args);
    return this;
  }

  withSwapStorage(
    ...args: Parameters<LegacyClientBuilder["withSwapStorage"]>
  ): this {
    this.#inner.withSwapStorage(...args);
    return this;
  }

  withMnemonic(...args: Parameters<LegacyClientBuilder["withMnemonic"]>): this {
    this.#inner.withMnemonic(...args);
    return this;
  }

  withXprv(...args: Parameters<LegacyClientBuilder["withXprv"]>): this {
    this.#inner.withXprv(...args);
    return this;
  }

  withAa(...args: Parameters<LegacyClientBuilder["withAa"]>): this {
    this.#inner.withAa(...args);
    return this;
  }

  withLogger(...args: Parameters<LegacyClientBuilder["withLogger"]>): this {
    this.#inner.withLogger(...args);
    return this;
  }

  withLogLevel(...args: Parameters<LegacyClientBuilder["withLogLevel"]>): this {
    this.#inner.withLogLevel(...args);
    return this;
  }

  async build(): Promise<Client> {
    return new Client(await this.#inner.build(), {
      enabled: this.#trackingEnabled,
      serverUrl: this.#serverUrl,
      managers: this.#managers,
      arkadeServerUrl: this.#arkadeServerUrl,
      esploraUrl: this.#esploraUrl,
      electrumWsUrl: this.#electrumWsUrl,
      bitcoinNetwork: this.#bitcoinNetwork,
      bitcoinMinConfirmations: this.#bitcoinMinConfirmations,
      evmRpcUrls: this.#evmRpcUrls,
      autoClaim: this.#autoClaim,
      chainVerified: this.#chainVerified,
    });
  }
}

/**
 * Stored statuses where the client's money is fully settled — already received,
 * already refunded, or (for `expired`) never deposited: the server only marks a
 * swap expired while it is unfunded (a late-funded one becomes
 * `clientfundedtoolate`, which is NOT in this set). Safe to skip registering
 * entirely, so tracking doesn't re-scan every historical swap on each start;
 * anything ambiguous stays chain-verified.
 */
/** The legacy client's resolved base URL, or `undefined` if it can't provide one. */
function legacyBaseUrl(legacy: LegacyClient): string | undefined {
  try {
    return legacy.baseUrl;
  } catch {
    return undefined;
  }
}

const SETTLED_STORED_STATUSES = new Set<SwapStatus>([
  "serverredeemed",
  "clientrefunded",
  "clientrefundedserverfunded",
  "clientrefundedserverrefunded",
  "clientredeemedandclientrefunded",
  "expired",
]);

/**
 * The swap id from a create result. Every `create*` result carries the created
 * swap under `response.id`; used to find the just-created swap in storage.
 */
function createdSwapId(result: unknown): string | undefined {
  const id = (result as { response?: { id?: unknown } } | null)?.response?.id;
  return typeof id === "string" ? id : undefined;
}
