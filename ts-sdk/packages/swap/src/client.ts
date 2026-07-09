/**
 * @satora/swap — the Satora swap client.
 *
 * A standalone `Client` with the same public surface as the legacy
 * `@lendasat/lendaswap-sdk-pure` client, so it is a drop-in replacement. For
 * now every legacy method is forwarded to an internally-owned legacy client
 * instance (see the `@deprecated` delegators below). New Satora-native features
 * (e.g. the action model from issue #848) are added directly on this class, and
 * individual delegators get replaced with native implementations over time.
 */
import {
  Client as LegacyClient,
  type ClientBuilder as LegacyClientBuilder,
} from "@lendasat/lendaswap-sdk-pure";

export class Client {
  /** The wrapped legacy client. Calls are forwarded here until migrated. */
  readonly #legacy: LegacyClient;

  /** @internal Use {@link Client.builder} instead of constructing directly. */
  constructor(legacy: LegacyClient) {
    this.#legacy = legacy;
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
    return this.#legacy.createSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createArkadeToEvmSwapGeneric(
    ...args: Parameters<LegacyClient["createArkadeToEvmSwapGeneric"]>
  ): ReturnType<LegacyClient["createArkadeToEvmSwapGeneric"]> {
    return this.#legacy.createArkadeToEvmSwapGeneric(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createLightningToEvmSwapGeneric(
    ...args: Parameters<LegacyClient["createLightningToEvmSwapGeneric"]>
  ): ReturnType<LegacyClient["createLightningToEvmSwapGeneric"]> {
    return this.#legacy.createLightningToEvmSwapGeneric(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createBitcoinToEvmSwap(
    ...args: Parameters<LegacyClient["createBitcoinToEvmSwap"]>
  ): ReturnType<LegacyClient["createBitcoinToEvmSwap"]> {
    return this.#legacy.createBitcoinToEvmSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createBitcoinToArkadeSwap(
    ...args: Parameters<LegacyClient["createBitcoinToArkadeSwap"]>
  ): ReturnType<LegacyClient["createBitcoinToArkadeSwap"]> {
    return this.#legacy.createBitcoinToArkadeSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createLightningToArkadeSwap(
    ...args: Parameters<LegacyClient["createLightningToArkadeSwap"]>
  ): ReturnType<LegacyClient["createLightningToArkadeSwap"]> {
    return this.#legacy.createLightningToArkadeSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createArkadeToLightningSwap(
    ...args: Parameters<LegacyClient["createArkadeToLightningSwap"]>
  ): ReturnType<LegacyClient["createArkadeToLightningSwap"]> {
    return this.#legacy.createArkadeToLightningSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  getArkadeToLightningQuote(
    ...args: Parameters<LegacyClient["getArkadeToLightningQuote"]>
  ): ReturnType<LegacyClient["getArkadeToLightningQuote"]> {
    return this.#legacy.getArkadeToLightningQuote(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  retryArkadeToLightningSwap(
    ...args: Parameters<LegacyClient["retryArkadeToLightningSwap"]>
  ): ReturnType<LegacyClient["retryArkadeToLightningSwap"]> {
    return this.#legacy.retryArkadeToLightningSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createEvmToArkadeSwapGeneric(
    ...args: Parameters<LegacyClient["createEvmToArkadeSwapGeneric"]>
  ): ReturnType<LegacyClient["createEvmToArkadeSwapGeneric"]> {
    return this.#legacy.createEvmToArkadeSwapGeneric(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createEvmToBitcoinSwap(
    ...args: Parameters<LegacyClient["createEvmToBitcoinSwap"]>
  ): ReturnType<LegacyClient["createEvmToBitcoinSwap"]> {
    return this.#legacy.createEvmToBitcoinSwap(...args);
  }

  /** Delegated to the legacy client (migration checkpoint). */
  createEvmToLightningSwapGeneric(
    ...args: Parameters<LegacyClient["createEvmToLightningSwapGeneric"]>
  ): ReturnType<LegacyClient["createEvmToLightningSwapGeneric"]> {
    return this.#legacy.createEvmToLightningSwapGeneric(...args);
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
}

/**
 * Builds a {@link Client}. Mirrors the legacy `ClientBuilder` fluent surface and
 * constructs the legacy client internally, then wraps it.
 */
export class ClientBuilder {
  readonly #inner: LegacyClientBuilder = LegacyClient.builder();

  withBaseUrl(...args: Parameters<LegacyClientBuilder["withBaseUrl"]>): this {
    this.#inner.withBaseUrl(...args);
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

  withEsploraUrl(
    ...args: Parameters<LegacyClientBuilder["withEsploraUrl"]>
  ): this {
    this.#inner.withEsploraUrl(...args);
    return this;
  }

  withArkadeServerUrl(
    ...args: Parameters<LegacyClientBuilder["withArkadeServerUrl"]>
  ): this {
    this.#inner.withArkadeServerUrl(...args);
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
    return new Client(await this.#inner.build());
  }
}
