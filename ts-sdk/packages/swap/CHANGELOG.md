# @satora/swap

## 1.4.0

### Minor Changes

- 151af66: Add EVM → Lightning swaps: `createEvmToLightningSwap` (invoice, lightning address or LNURL destination, funded from USDC/WBTC/tBTC on Polygon, Ethereum or Arbitrum) with tracking, and `createSwap` routing to it for Lightning destinations.

  `getLightningSendQuote` now accepts EVM sources via `sourceChain` + `sourceToken`. `sourceAmount` (the source token's smallest unit, as a string in the result) replaces `sourceAmountSats`, which stays as a deprecated alias: accepted as a param, and set on Arkade-source results.

  Tracking: a swap paying out over Lightning no longer surfaces a refund while the server's payment is in flight (`serverfunded`); the derived action is a wait, with the refund listed but blocked until the deposit's own timelock.

  `fundSwap` reports the source amount the server quoted for the attempt through a new `onQuote` option (an EVM → Lightning swap is priced afresh on every funding) and refreshes the stored swap once the funding lands. A funding that would revert now throws `SimulationRevertError`, carrying the revert reason, instead of a plain `Error`.

### Patch Changes

- 6721be5: Target backend 0.3.13 in the x-satora-server-version header.
- 6c428fc: Target backend 0.3.14 in the x-satora-server-version header.

## 1.3.3

### Patch Changes

- f401036: Target backend 0.3.12 in the x-satora-server-version header.
- b8e0b74: Pick up `@lendasat/lendaswap-sdk-pure`: every Arbitrum EVM-target claim is
  published as a paymaster-sponsored UserOp when AA config is present.

## 1.3.2

### Patch Changes

- 0f7f94f: Target backend 0.3.10 in the x-satora-server-version header and align the
  Arkade SDK dependency on 0.4.66.
- 6817e60: Target backend 0.3.11 in the x-satora-server-version header.

## 1.3.1

### Patch Changes

- e198470: Target backend 0.3.8 in the x-satora-server-version header.
- 7771b28: Target backend 0.3.9 in the x-satora-server-version header.
- 1997ca0: Pick up the pure SDK's baked-in default account-abstraction config (bundler
  URL + Gas Manager policy): CCTP-inbound swaps and sponsored UserOp claims work
  out of the box; `withAa()` still overrides.

## 1.3.0

### Minor Changes

- bc8011c: Exact Arkade → Lightning quotes: `getLightningSendQuote` and the
  `lightningDestination` option on `getQuote` are delegated from the pure
  SDK — quotes with a concrete destination (invoice / lightning address /
  LNURL) carry the provider's real Lightning send fee instead of the flat
  estimate.
- 8ef73bb: Lightning → EVM swaps: `createLightningToEvmSwap` delegation, tracker
  mapping (EVM server leg, no client leg), and action derivation (no refund
  action — the hold payment unwinds on its own).

## 1.2.1

### Patch Changes

- 36fab34: Target backend 0.3.6 in the x-satora-server-version header.
- 493a61a: Target backend 0.3.7 in the x-satora-server-version header.

## 1.2.0

### Minor Changes

- 7678afc: Tracking now runs on server status hints by default, with zero chain access
  (TEMP while the chain monitors are being fixed). A new `HintTracker` feeds the
  server's pushed `SwapStatus` straight into the action derivation; timelocks are
  evaluated against the wall clock. Chain-verified tracking remains available via
  `ClientBuilder.withChainVerifiedTracking()` and is still implied by
  `withContractManagers`.

## 1.1.0

### Minor Changes

- d50ee09: Arkade→Lightning swaps, rebuilt on the Spark provider.

  - `createArkadeToLightningSwap` is delegated to the underlying client
    (see `@lendasat/lendaswap-sdk-pure` for the API shape) and created
    swaps are tracked: the client-funded Arkade VHTLC leg is watched; the
    Lightning payout has no on-chain leg.
  - `refundSwap()` handles the direction via the shared Arkade VHTLC
    collaborative-refund flow.

- 9f3bb60: Removed the swap-back refund mode. EVM-sourced swaps now always refund
  the BTC-pegged HTLC token (tBTC/WBTC) directly to the depositor.

  The delegated refund methods (`refundSwap`, `refundEvmWithSigner`,
  `collabRefundEvmSwap`, `collabRefundEvmWithSigner`,
  `submitCollabRefundEvm`) inherit the new signatures from
  `@lendasat/lendaswap-sdk-pure` — the `mode`/`settlement` parameter is
  gone.

### Patch Changes

- f96e1f3: Target backend 0.3.5 in the x-satora-server-version header.
- e389209: Tracking readers (Electrum and Esplora) now select the funding candidate as
  the transaction paying the MOST to the HTLC address instead of the first one
  listed. The address is public, so a stray dust payment could previously be
  mistaken for the funding — observing the swap as underfunded/invalid and
  suppressing auto-claim while the real deposit sat at the address.

## 1.0.0

### Major Changes

- 6866996: Lightning v2 (Spark provider), clean wire break.

  - `@satora/swap`: Lightning→Arkade tracking now reads the generic
    `target_amount` field; wrappers for the removed Arkade→Lightning and
    Lightning↔EVM directions are gone until those flows are rebuilt on the
    new provider.
  - `@satora/escrow-client`: `fundFromLightning` uses `targetAmountSats`;
    `withdrawToLightning` / `quoteLightningWithdrawal` keep their signatures
    but throw `LightningWithdrawalUnavailableError` while Arkade→Lightning
    swaps are rebuilt.

### Minor Changes

- 473eac2: Electrum-backed Bitcoin chain reader with push reconciles, on by default for
  mainnet (Satora's Fulcrum; `withElectrumWsUrl()` overrides, other networks
  stay Esplora-only unless a URL is set). The tracker's Bitcoin manager reads
  HTLC state from the Electrum server (fresher than public explorers) and
  re-verifies the moment an address's history changes via
  `blockchain.scripthash.subscribe` — so a server-funded swap claims in seconds
  instead of waiting on the poll cadence. The Esplora reader remains the
  fallback on Electrum errors. Non-mainnet deployments set
  `withBitcoinNetwork()` so Electrum address decoding uses the right parameters.

### Patch Changes

- 64e8902: Scan every Arkade spend PSBT input when extracting a matching preimage, fixing classification for multi-input VHTLC spends.
- 8dbb24f: Target backend 0.3.2 in the x-satora-server-version header.
- b77fbf9: Target backend 0.3.3 in the x-satora-server-version header.
- 1068ea3: Target backend 0.3.4 in the x-satora-server-version header.
- e305ec8: Update Arkade VHTLC tracking helpers for the strict VHTLC script version and byte-parity vectors shared with the backend and SDKs.

## 0.3.2

### Patch Changes

- 9153ac2: Fix the x-satora-server-version header value: the previous release shipped `lendaswap@0.3.1` (not valid semver), which the server rejects. Now sends `0.3.1`.

## 0.3.1

### Patch Changes

- 75f4743: Target backend lendaswap@0.3.1 in the x-satora-server-version header.

## 0.3.0

### Minor Changes

- 573eec6: Chain-derived swap tracking.

  - `WaitAction` gains `waitingOn`
    (`"client_payment" | "client_funding_confirmation" | "server_funding" | "claim_confirmation" | "refund_timelock"`)
    and terminal `NoneAction` gains `outcome`
    (`"completed" | "refunded" | "expired"`). Tracker emissions also carry
    `observations` — the raw chain facts the actions were derived from.
  - Tracking is now push-driven via the server's status WebSocket (observe mode
    included), with each pushed transition verified on-chain; background polling
    is gone. While client funds are locked on-chain, an independent watcher
    re-verifies both legs every 60s, so refund availability never depends on the
    server.
  - Bitcoin funding is treated as confirmed at 0-conf by default, so
    evm→bitcoin swaps claim immediately.
    `ClientBuilder.withBitcoinMinConfirmations(n)` restores block-depth
    policies.
  - `startTracking()` is safe to call concurrently; settled swaps persist their
    final status so later sessions skip them; a swap that stays unfunded past
    its refund locktime derives a terminal `expired`.

- 31484e1: Client-funded Bitcoin HTLCs (`bitcoin_to_evm`, `btc_to_arkade`) now observe as
  `mempool` until confirmed, matching when the server acts on them — so
  `waitingOn: "client_funding_confirmation"` is actually reported instead of the
  swap jumping straight to "server funding". Server-funded legs
  (`evm_to_bitcoin`) remain claimable at 0-conf. `HtlcRef` carries an optional
  per-leg `minConfirmations` for Bitcoin legs; the reader-wide default still
  applies where a ref sets none.
- 1c027f8: Read the `key` field on `HTLCErc20` lifecycle events. The event signatures
  change, so this release reads a `VERSION 4` contract and earlier releases do
  not — ship it together with the contract it reads.

## Unreleased

### Patch Changes

- Send `x-satora-server-version` on API requests via the underlying pure SDK. The header is the semver server/API version the SDK was built against.

## 0.2.0

### Minor Changes

- e2271fe: Expose refunded EVM-source swap continuation APIs through the migrated swap SDK. `getRefundedEvmSwapContinuation` reports whether a refunded EVM-source swap has recoverable Kernel-account balance, and `continueRefundedEvmSwap` creates and funds a replacement swap from that balance.
- 9d35eee: Hint-driven auto-claim and more resilient tracking.

  - **Opt-in auto-claim** via `ClientBuilder.withAutoClaim({ onActionRequired? })`. When enabled, tracking also subscribes to the server's status WebSocket (a faster trigger than the chain poll) and, once the chain confirms a swap is claimable, claims it automatically. Actions that need the user — a manual `fund`, or a refund to confirm — are surfaced through `onActionRequired` instead of being run. Off by default, since it spends on the user's behalf.
  - **Track-on-create**: a swap created after `startTracking` is now folded into tracking (and auto-claimed) without a restart — every `create*` method routes through the hook.
  - **Unreachable chains no longer break tracking**: a swap with a leg on an unconfigured chain (e.g. an EVM chain with no RPC) is skipped at `startTracking` instead of aborting tracking for every other swap, and `create*` throws rather than letting you fund a swap that can be neither observed nor claimed.

  **Breaking:** `ContractManager` now requires a `canObserve(ref)` method. Only affects code that implements `ContractManager` directly (e.g. via `ClientBuilder.withContractManagers`); the built-in managers already provide it.

  Also fixes: the WebSocket subscription cap is now enforced client-side (overflow was silently dropped by the server), `SwapTracker.track()` rolls back a partial registration on failure, and the worker no longer surfaces the no-op `wait` action as one needing attention.

## 0.1.0

### Minor Changes

- 1db87ef: `@satora/swap` is now a standalone, drop-in swap client instead of a bare
  re-export of `@lendasat/lendaswap-sdk-pure`. `Client` and `ClientBuilder` wrap
  the underlying legacy client with the exact same public API, and this is where
  new Satora-native features will land.

  **This is the recommended swap package going forward.** We intend to deprecate
  `@lendasat/lendaswap-sdk-pure` and migrate all consumers over to `@satora/swap`.
  Migrating is a drop-in change — swap the package name in your imports, nothing
  else changes. The legacy package stays supported throughout the transition.

- 80b3047: Add a derived next-action model with observe-mode tracking, so consumers no
  longer have to re-infer UX from the raw 16-state `SwapStatus`.

  Call `client.startTracking()` and subscribe with `client.subscribeToActions(cb)`
  to receive the recommended next action for each of your swaps — `fund`, `wait`,
  `claim`, `refund`, or `none` — recomputed as the chain state changes. The state
  is derived **purely from on-chain observations** (per-ledger contract managers
  watching each HTLC), never from the server's status, so it also works for
  recovery when the API is unavailable. Each leg's funding amount, token and
  recipient are verified, so the client is never told to claim a leg funded on the
  wrong terms.

  Covers every swap direction: Arkade↔EVM, Bitcoin↔EVM, `btc_to_arkade`, and all
  four Lightning directions. Tracking is on by default (with sensible RPC, esplora,
  and Arkade endpoints) and is overridable or disableable via the `Client` builder.

## 0.0.5

### Patch Changes

- 9f4d595: Add package READMEs for `@satora/escrow` and `@satora/swap`.
- bbba274: Add package README (published to npm).

## 0.0.5-rc.0

### Patch Changes

- 9f4d595: Add package READMEs for `@satora/escrow` and `@satora/swap`.
- bbba274: Add package README (published to npm).
