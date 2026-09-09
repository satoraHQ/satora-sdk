# @lendasat/lendaswap-sdk-pure

## 2.3.0

### Minor Changes

- 151af66: Add EVM → Lightning swaps: `createEvmToLightningSwap` (invoice, lightning address or LNURL destination, funded from USDC/WBTC/tBTC on Polygon, Ethereum or Arbitrum), `createSwap` routing to it for Lightning destinations, and refund / Permit2 / gasless support for the new direction.

  `getLightningSendQuote` now accepts EVM sources via `sourceChain` + `sourceToken`. `sourceAmount` (the source token's smallest unit, as a string in the result) replaces `sourceAmountSats`, which stays as a deprecated alias: accepted as a param, and set on Arkade-source results.

  `fundSwap` reports the source amount the server quoted for the attempt through a new `onQuote` option (an EVM → Lightning swap is priced afresh on every funding) and refreshes the stored swap once the funding lands. A funding that would revert now throws `SimulationRevertError`, carrying the revert reason, instead of a plain `Error`.

### Patch Changes

- 6721be5: Target backend 0.3.13 in the x-satora-server-version header.
- 6c428fc: Target backend 0.3.14 in the x-satora-server-version header.

## 2.2.0

### Minor Changes

- b8e0b74: Publish every Arbitrum EVM-target claim as a paymaster-sponsored UserOp when
  AA config is present (`.withAa(...)`), not only DEX/CCTP claims. Plain
  (no-DEX) claims fall back to the server-submitted `/claim-gasless` path if
  the UserOp publish fails or no AA config is set; DEX/CCTP claims still
  require AA config.

### Patch Changes

- f401036: Target backend 0.3.12 in the x-satora-server-version header.

## 2.1.2

### Patch Changes

- 0f7f94f: Target backend 0.3.10 in the x-satora-server-version header and align the
  Arkade SDK dependency on 0.4.66.
- 6817e60: Target backend 0.3.11 in the x-satora-server-version header.

## 2.1.1

### Patch Changes

- e198470: Target backend 0.3.8 in the x-satora-server-version header.
- 7771b28: Target backend 0.3.9 in the x-satora-server-version header.
- 1997ca0: Bake the default account-abstraction config (bundler URL + Gas Manager policy)
  into published builds, matching the hosted frontend. CCTP-inbound swaps and
  sponsored UserOp claims now work out of the box; `withAa()` still overrides.

## 2.1.0

### Minor Changes

- bc8011c: Exact Arkade → Lightning quotes. New `getLightningSendQuote` hits
  `/quote/lightning-send` with a concrete destination (BOLT11 invoice,
  lightning address or LNURL) and returns amounts priced with the
  provider's real Lightning send fee — which the server now charges as the
  swap's network fee instead of a config-flat estimate. `getQuote` accepts
  an optional `lightningDestination` and serves Arkade → Lightning quotes
  through the exact endpoint when it is set, falling back to the estimate
  on failure.
- 8ef73bb: Lightning → EVM swaps: `createLightningToEvmSwap` (and `createSwap` with a
  Lightning source + EVM target) pays a hold invoice and receives any
  1inch-reachable ERC-20, with gasless claims and CCTP/USDT0 bridge
  destinations supported. Claims reuse the existing EVM-targeted flow; there
  is no client refund action — an unclaimed hold payment unwinds on its own.

### Patch Changes

- 6d62269: Remove the deprecated VTXO refresh swap API from generated types.

## 2.0.1

### Patch Changes

- 36fab34: Target backend 0.3.6 in the x-satora-server-version header.
- 493a61a: Target backend 0.3.7 in the x-satora-server-version header.
- 52e29d6: `buildRedeemDigest` accepts an optional `htlcVersion` (the swap response's
  `evm_htlc_version`, defaulting to 4) so EIP-712 redeem signatures keep
  matching the HTLCErc20 deployment a swap was created on across contract
  upgrades. The gasless and userop claim paths thread it automatically.

## 2.0.0

### Major Changes

- 9f3bb60: Removed the swap-back refund mode. EVM-sourced swaps now always refund
  the BTC-pegged HTLC token (tBTC/WBTC) directly to the depositor; the
  DEX swap back to the original source token is gone.

  Breaking API changes:

  - `refundSwap()`'s `EvmRefundOptions` no longer has `mode`.
  - `refundEvmWithSigner`, `collabRefundEvmSwap`,
    `collabRefundEvmWithSigner`, `getCollabRefundEvmParams`, and
    `buildCollabRefundEvmTypedData` lost their `mode`/`settlement`
    parameter.
  - `submitCollabRefundEvm`'s body no longer takes `mode`, `sweep_token`,
    or `min_amount_out` — the server hardcodes `sweepToken` = tBTC/WBTC
    and `minAmountOut` = 0.
  - `CollabRefundEvmParams` lost `mode`, `sourceTokenAddress`, and
    `dexCalldata`; the unused `CoordinatorRefundCallData` type was
    removed.

  Old 1.x clients keep working against the new server: the removed fields
  are ignored and refunds degrade to direct settlement.

### Minor Changes

- d50ee09: Arkade→Lightning swaps, rebuilt on the Spark provider.

  - New `createArkadeToLightningSwap` (and a `createSwap` dispatcher route):
    destination is one of `lightningInvoice` (its amount pins the payout),
    or `lightningAddress`/`lnurl` with exactly one of `sourceAmountSats`
    (send-max, fees deducted from the payout) or `targetAmountSats` (exact
    payout, fees added on top). The swap's hash lock is the invoice's
    payment hash; the derived swap key only signs refunds.
  - `refundSwap()` and `amountsForSwap()` accept `arkade_to_lightning`
    swaps — collaborative refund first (server cosigns, no locktime wait),
    unilateral fallback.
  - Fee model: the user pays `payout + protocol fee + flat network fee`;
    the provider's actual Lightning fee is paid out of that margin, so
    quote and create can never disagree. `SwapPairInfo` gains
    `network_fee_sats`, and `composeQuote()` folds it into `network_fee`.

### Patch Changes

- f96e1f3: Target backend 0.3.5 in the x-satora-server-version header.

## 1.0.0

### Major Changes

- 6866996: Lightning v2 (Spark provider), clean wire break.

  - `createLightningToArkadeSwap` now takes exactly one of `sourceAmountSats`
    (invoice amount, fees deducted) or `targetAmountSats` (exact Arkade
    receive amount) instead of `satsReceive`, and the response uses the
    generic `source_amount`/`target_amount`/`source_token`/`target_token`
    fields (no more `boltz_*`, `lightning_expected_sats`, or `sats_receive`).
  - Removed until they are rebuilt on the new provider:
    `createArkadeToLightningSwap`, `retryArkadeToLightningSwap`,
    `getArkadeToLightningQuote`, `createLightningToEvmSwapGeneric`,
    `createEvmToLightningSwapGeneric`, `collabRefundArkadeToLightningOffchain`,
    and the corresponding `createSwap` dispatcher routes.

### Minor Changes

- 473eac2: Electrum-over-WebSocket support for Bitcoin chain access. On mainnet the
  client now defaults to Satora's Fulcrum (`wss://electrs.satora.io`, see
  `DEFAULT_ELECTRUM_WS_URLS`); `withElectrumWsUrl()` overrides it, and other
  networks stay Esplora-only unless a URL is set. HTLC output lookups and
  broadcasts prefer Electrum over Esplora, and waiting for an HTLC funding
  becomes push-driven via `blockchain.scripthash.subscribe` instead of polling.
  Esplora remains the automatic fallback whenever the Electrum server errors.
  Address UTXO lookups (both backends) now select the largest output instead of
  the explorer's first, so a stray dust output at the public HTLC address can no
  longer shadow the real deposit.

### Patch Changes

- 8dbb24f: Target backend 0.3.2 in the x-satora-server-version header.
- b77fbf9: Target backend 0.3.3 in the x-satora-server-version header.
- 1068ea3: Target backend 0.3.4 in the x-satora-server-version header.
- a5330b7: Serialize EVM token amounts as decimal strings so 18-decimal token values can exceed JavaScript's safe integer range.
- d8e4232: Support large EVM token quote amounts as decimal strings.
- e305ec8: Make strict Arkade VHTLC script construction byte-identical to the backend and add cross-language vectors for the scripts and address.

## 0.6.2

### Patch Changes

- 9153ac2: Fix the x-satora-server-version header value: 0.6.1 shipped `lendaswap@0.3.1` (not valid semver), which the server rejects. Now sends `0.3.1`.

## 0.6.1

### Patch Changes

- 75f4743: Target backend lendaswap@0.3.1 in the x-satora-server-version header.

## 0.6.0

### Minor Changes

- 68a0db7: Sign `HTLCErc20` EIP-712 payloads against domain version `"4"`, matching the
  contract's `VERSION` bump. This release must ship together with the v4
  contract. `HTLCCoordinator` is a separate domain and stays on `"3"`.
- 6b830dd: `CreateSwapOptions` gains `bridgeRecipient` (the destination USDC ATA) and
  `bridgeRecipientWallet` (the owning wallet, only when the ATA still needs
  creation). Both are persisted on the `StoredSwap`, so a bare `claim(swapId)`
  now works for BTC→USDC-on-Solana (CCTP) swaps; explicit claim options still
  take precedence. Also pins `dexie` to an exact version to avoid
  duplicate-instance errors in monorepos.

### Patch Changes

- 6f866d2: Gate the on-chain Bitcoin refund on the chain's median time past instead of
  the local clock, so refund availability matches what the chain actually
  accepts.

## Unreleased

### Patch Changes

- Add `x-satora-server-version` to SDK API requests. The header is the semver server/API version the SDK was built against.

## 0.5.0

### Minor Changes

- 70976d8: Add Esplora fallback URLs for Bitcoin lookups and broadcasts. Mainnet now defaults to mempool.space with blockstream.info as fallback, tried in order. `withEsploraUrl` / `ClientConfig.esploraUrl` accept a list of URLs. Requests carry per-endpoint timeouts (2s lookups, 10s broadcasts) so a hung explorer fails over instead of stalling the claim/refund flow.
- e2271fe: Add SDK support for continuing refunded EVM-source swaps. New APIs expose continuation eligibility, detect refunded balances in the Kernel account, create a replacement EVM-to-Arkade, EVM-to-Bitcoin, or EVM-to-Lightning swap, and submit the replacement funding UserOp from the recovered balance.

  Add a balance-funding CCTP inbound UserOp path for swaps whose funds are already in the Kernel account. This skips `receiveMessage` and submits the `approve(Permit2) + executeAndCreateWithPermit2` batch.

  Support split account-abstraction endpoints. `AaConfig` now accepts an optional `rpcUrl` for normal chain reads while `bundlerUrl` is used for UserOps, and `paymasterPolicyId` is optional so callers can send self-funded UserOps when no paymaster is configured.

### Patch Changes

- ea456e6: Improve the collaborative EVM refund error when the recorded depositor address is missing.
- 10c95ce: Validate Arkade addresses before creating swaps. `createBitcoinToArkadeSwap`, `createLightningToArkadeSwap`, and `createEvmToArkadeSwapGeneric` now throw early on a malformed target address instead of sending it to the server. Adds `parseArkadeAddress` (returns the decoded `ArkAddress`) and `isValidArkadeAddress` helpers (full bech32m decode, optional network check).

## 0.4.0

### Minor Changes

- e42e8c8: Add an optional `invoiceDescription` to `createLightningToEvmSwapGeneric` and `createLightningToArkadeSwap`.

  Sets the text shown in the payer's wallet when they open the Lightning invoice. When omitted, the server applies a branded default (e.g. `Satora swap to USDC on Optimism`); an explicit empty string blanks the description. Backed by the new optional `invoice_description` field on the `POST /swap/lightning/evm` and `POST /swap/lightning/arkade` endpoints.

### Patch Changes

- 1db87ef: Recommend `@satora/swap` for new code. This is now the legacy SDK — it stays
  fully supported, but `@satora/swap` is a drop-in replacement (same API; just
  change the package name in your imports) and is where new features land. We
  intend to deprecate this package and migrate consumers over to `@satora/swap`.
- eb07502: Regenerate the OpenAPI types (adding the `btc_to_arkade` and Lightning swap
  response fields) and align `@arkade-os/sdk` to `^0.4.45`.
- 57a0d76: Fix gasless EVM collaborative refunds to sign with the actual coordinator depositor key.

## 0.3.0

### Minor Changes

- 43a6fc7: Add `getBulkStatus(ids)` to fetch the status of many swaps in a single request.

  Returns `{ statuses, not_found }` — only each swap's status, so the whole batch is served by one database query. Unknown IDs are returned in `not_found` instead of throwing, so one bad ID does not fail the whole call. Backed by the new `POST /swap/bulk-status` endpoint (max 100 IDs per request).

- 9f4d595: Add support for EURe in Arbitrum.
  SDK uses new orchestration flow.

### Patch Changes

- 0fb68c7: Export the real SDK version from the package entry point. `SDK_VERSION`, `SDK_NAME`, and `CLIENT_AGENT` are now re-exported from the index (sourced from the generated `version.ts`); the stale hard-coded `VERSION = "0.0.1"` export is removed.
- ed3d6d8: Export `SDK_COMMIT_HASH` — the git commit the SDK was built from. It's injected into the generated `version.ts` at build time from the `GIT_COMMIT_HASH` env var (set in CI on publish, same convention as the backend), and defaults to `"unknown"` for local builds. Lets consumers report the exact SDK source revision (e.g. in a version footer).

## 0.3.0-rc.2

### Patch Changes

- ed3d6d8: Export `SDK_COMMIT_HASH` — the git commit the SDK was built from. It's injected into the generated `version.ts` at build time from the `GIT_COMMIT_HASH` env var (set in CI on publish, same convention as the backend), and defaults to `"unknown"` for local builds. Lets consumers report the exact SDK source revision (e.g. in a version footer).

## 0.3.0-rc.1

### Patch Changes

- 0fb68c7: Export the real SDK version from the package entry point. `SDK_VERSION`, `SDK_NAME`, and `CLIENT_AGENT` are now re-exported from the index (sourced from the generated `version.ts`); the stale hard-coded `VERSION = "0.0.1"` export is removed.

## 0.3.0-rc.0

### Minor Changes

- 9f4d595: Add support for EURe in Arbitrum.
  SDK uses new orchestration flow.
