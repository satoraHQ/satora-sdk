# @satora/swap

## 1.10.1

### Patch Changes

- 3458cbf: Escrow signing accepts the PSBTs the ark-escrow gem builds again.
  `signEscrowArkTx`, `signEscrowCheckpoints` (and in `@satora/swap` also
  `signEscrowDelegate` and `getArkTxid`) threw
  `checkScript: witnessScript without P2WSH`, because ark-core records the spend
  script as `witnessScript` on taproot inputs and `@scure/btc-signer` 2.4 rejects
  that. The helpers now drop that field from taproot inputs and parse the result
  with all checks on. The signed PSBT they return no longer carries it.

## 1.10.0

### Minor Changes

- c4af4d0: Add `Asset.CIRBTC_ETHEREUM` for Circle Wrapped Bitcoin on Ethereum and recognize cirBTC as a BTC-pegged asset for BTC/sats display.
- 44a5ced: USDC payouts on Ethereum and Polygon settle on Arbitrum and bridge over CCTP. `getQuote`, `composeQuote` and `createSwap` route a swap whose target is native USDC on either chain the way they route USDC on Base: the HTLC locks tBTC on Arbitrum, the claim swaps to Arbitrum USDC, and Circle's Forwarding Service mints it on the destination. The quote's `bridge_fee` carries the CCTP fee. Swaps funded from USDC on these chains, and every other token on them, still run on the chain directly.

  New helpers: `isCctpRoutedUsdcTarget(chain, token)` and `isBridgedTarget(chain, token)`, which is true for any target that settles on Arbitrum and bridges.

  `createSwap` now refuses a source-pinned CCTP payout whose quoted net output after the bridge fee is not positive. Before, such a swap could be funded and then never claimed, because only the claim checks the fee against the output. Circle's forwarding fee to Ethereum is over a dollar.

### Patch Changes

- 9d125fb: Arkade to Lightning and Bitcoin to Lightning creation, and recovery of a swap
  not stored before, check the lock address the user funds against one rebuilt
  from the invoice's payment hash and the swap's refund key (see the legacy SDK
  changeset).
- 800aa2f: Arkade claims, refunds, collaborative refunds and escrow releases now submit in one call (one-shot submit): checkpoints are signed up front and sent with the ark tx. Older Arkade servers still get a separate `finalizeTx`. `submitAndFinalizeEscrowRelease` now matches the server's checkpoints by txid instead of position.
- 42cc809: Use the current `@arkade-os/sdk` names (`OperatorProvider`, `RestOperatorProvider`, `Proof`, `SignedProof`, `OperatorInfo`) instead of their deprecated aliases. Type-only; no behaviour change.
- e926b23: Bump `@arkade-os/sdk` to 0.5.5.
- 6ec950a: Target backend 0.3.26 in the x-satora-server-version header.
- dc9cca9: Claims pay out to the target the swap was created with: creation refuses a
  response naming another target or claiming address, the target is stored
  with the swap, and a server refresh can no longer change a stored swap's
  payout addresses. SQLite storage now keeps the target and bridge recipient
  (see the legacy SDK changeset).
- 0e154f7: A direct EVM claim refuses server calls (see the legacy SDK changeset).
- 3fa9a4c: Type the new fields `GET /swap/{id}` returns for EVM-sourced swaps (`evm_to_bitcoin`, `evm_to_lightning` and `evm_to_arkade`): `evm_htlc_locked_amount` (the amount the HTLC holds, as observed from its `SwapCreated` event) and `evm_htlc_sender` (the address the lock is keyed under), both `null` until the lock is seen. `evm_htlc_token` names the token locked in the HTLC on all three; `wbtc_address`, which carried the same value, is deprecated but still sent (and now also on `evm_to_arkade`). The SDK reads the lock token from `evm_htlc_token` (falling back to `wbtc_address` only for swaps stored before the server sent it), so relay funding now also binds `evm_to_arkade` payloads to the lock token. With these, a REST client can build a unilateral refund without parsing the creation log.
- 5178b4f: Check that a Lightning invoice pays the swap's hash lock at creation and when
  recovery stores a swap for the first time, in every Lightning direction
  except EVM-sourced ones, and keep a stored swap's hash locks, lock addresses,
  invoice payment hashes and keys from being replaced by a server refresh or
  recovery (see the legacy SDK changeset). A refused refresh still updates the
  swap's status. `SwapVerificationError` is exported.

## 1.9.0

### Minor Changes

- 00e5463: Include the internal pure SDK's code and types in @satora/swap. Consumers no longer install @lendasat/lendaswap-sdk-pure, and pure changes ship with the swap package's version and release.

## 1.8.1

### Patch Changes

- 27edee2: Target backend 0.3.24 in the x-satora-server-version header.
- 835913f: Target backend 0.3.25 in the x-satora-server-version header.
- 71f5946: Re-read the server's HTLC 1, 2, 4 and 8 seconds after a status hint says it is funded while the chain read does not show it yet. The public RPCs trail the server's node by a few blocks, so the first read often missed the lock and the auto-claim waited for the next at-risk reconcile, up to a minute.
- a8b09c7: Track a swap created with `createLightningToEvmSwap`, like every other create method, so a client with auto-claim claims it without a restart.

## 1.8.0

### Minor Changes

- 34c7de3: `getCollabRefundEvmSignature` wrapper; collaborative refunds on HTLCErc20 v7 / HTLCNative v2 go through the depositor (see `@lendasat/lendaswap-sdk-pure`).
- 34c7de3: Declare EVM HTLC protocol epoch 2, so new swaps are created on HTLCErc20 v7 / HTLCCoordinator v5 / HTLCNative v2 (see `@lendasat/lendaswap-sdk-pure`).
- 34c7de3: `getRehashEvmParams`, `signRehashEvm`, `retrySwapWithInvoice` and `retrySwapWithInvoiceWithSigner` wrappers: retry a stuck EVM → Lightning swap with a new invoice (see `@lendasat/lendaswap-sdk-pure`).
- 8fc379b: Use direct HTLC funding and claims for Rootstock. Require the payout wallet to claim, using RIF Relay when configured and wallet-paid gas otherwise. Preserve existing gasless behavior on Ethereum, Arbitrum, and Polygon.
- cf2c740: Expose native USDT payouts to Tron through the wrapped SDK, including address validation and destination-specific delivery tracking.

### Patch Changes

- f61deb6: Bump `@arkade-os/sdk` to `^0.5.1` (pulls in the Arkade claim fix from `@lendasat/lendaswap-sdk-pure`). The `@arkade-os/sdk` peer range moves from `^0.4.66` to `^0.5.1`, which is breaking for consumers still on 0.4.

  `@satora/escrow` / `@satora/escrow-client`: `escrowArkConfigFromInfo` now takes the 0.5 `ArkadeInfo`, `dispose()` returns a `Promise<void>`, and wallet roles resolve from `PathContext.walletDescriptor` (bare x-only hex or `tr(<key>)`).

- e869e8a: Target backend 0.3.23 in the x-satora-server-version header.
- 34c7de3: Delegates `getEvmFundingExpectation(swapId, pins?)`: the hash lock and funding terms the client fixed for an EVM-sourced swap, to pass as `expected` when building CCTP-inbound calldata yourself (`buildCctpInboundBatch` now requires it).
- 34c7de3: Permit2 funding on HTLCErc20 v7 / HTLCNative v2 signs the witness with the depositor as refund address (see `@lendasat/lendaswap-sdk-pure`).
- 7dcbf9a: A funded EVM swap advances as soon as `fundSwap` resolves.

  With chain-verified tracking, `fundSwap` now re-verifies the swap against the
  chain once the funding transaction has mined, so the derived action moves off
  `fund` without waiting for the server's status push. On Rootstock that push
  follows a 30 s poll plus the configured confirmations, during which the
  deposit step looked stuck. Hints-only tracking is unchanged: it has no chain
  to read.

  A swap tracked after start is put on the hint feed when it is tracked, not
  only once it emits its first action; a swap the tracker already saw finish is
  not re-subscribed. On a chain whose public RPC serves no
  `eth_getLogs` an unfunded lock cannot be classified, so such a swap emitted
  nothing and never received the server's `clientfunded` hint.

- 34c7de3: `refundEvmSponsored` refunds an HTLCErc20 v7 lock with the smart account's own `refund`, which the paymaster sponsors, instead of `refundFor` (see `@lendasat/lendaswap-sdk-pure`).

## 1.7.2

### Patch Changes

- 5a982d2: Target backend 0.3.22 in the x-satora-server-version header.

## 1.7.1

### Patch Changes

- 4fbf4f9: Target backend 0.3.20 in the x-satora-server-version header.
- 4e23daf: Target backend 0.3.21 in the x-satora-server-version header.

## 1.7.0

### Minor Changes

- 2e4486b: Track Bitcoin (on-chain) ↔ Lightning swaps: the client-funded BTC HTLC of
  Bitcoin → Lightning is observed like the Bitcoin → EVM one, the
  server-funded HTLC of Lightning → Bitcoin like the EVM → Bitcoin one; the
  Lightning legs have no chain leg.

  On mainnet the tracker stops recommending a claim of a Lightning → Bitcoin
  HTLC 30 minutes before its refund locktime, so a slow claim cannot reveal the
  preimage to a racing refund.

- 180bb1f: `fundSwap` funds a CCTP-inbound swap (USDC from Base, Optimism, Avalanche, … to
  Bitcoin) from a browser wallet, records the burn to resume after an
  interruption, and `recoverCctpInboundSwap` recovers the USDC if settlement
  never lands (see the legacy SDK changeset).
- 535b1c8: Send USDC from Solana: `fundSwapFromSolana` burns USDC from a Solana wallet and
  settles the swap on Arbitrum, `getSolanaUsdcBalance` reads the wallet's USDC,
  and the builder gains `withSolanaRpcUrl` (see the legacy SDK changeset).
- 20d07cd: Swap a refunded EVM-sourced swap again instead of leaving the refund behind.
  `Client.refundEvmSponsored(swapId)` refunds a swap whose depositor is this client's
  key (created `gasless`, incl. CCTP-inbound) as a paymaster-sponsored UserOp on
  Arbitrum, no wallet or gas needed. `continueRefundedEvmSwap` then swaps the
  refund again: its `signer` is now optional (defaults to this client's key; any
  other owner is rejected before a swap is created), it
  takes a `target` (`"bitcoin" | "arkade" | "lightning"`, defaulting to the old
  swap's), and it can pay out to a Lightning invoice, Lightning address or LNURL.
  `getRefundedEvmSwapContinuation` accepts Lightning swaps and an optional signer.
- 8617e57: RBTC claims through RIF Relay: `getRifRelay(chainId)` and the RIF branch of
  `claimEvmWithSigner` (see the legacy SDK changeset). The auto-claim worker hands
  a swap that needs the user's wallet to `onActionRequired` once instead of
  retrying it.

### Patch Changes

- bda65c0: Target backend 0.3.18 in the x-satora-server-version header.
- 837b6ea: Target backend 0.3.19 in the x-satora-server-version header.
- 48e9524: `refundEvmSponsored` also refunds gasless swaps created by older SDK versions,
  whose depositor is the swap's own key rather than this client's key.
- c718a43: Outbound USDT0 quotes charge the LayerZero messaging fee in sats as part of
  the network fee (see the legacy SDK changeset).

## 1.6.1

### Patch Changes

- d83926d: Target backend 0.3.17 in the x-satora-server-version header.

## 1.6.0

### Minor Changes

- 033243b: Send one protocol compatibility epoch header per protocol component (`bitcoin-htlc`, `arkade-vhtlc`, `lightning`, `evm-erc20-htlc`, `evm-native-htlc`) on every request instead of the legacy server-version header.
- 208a6a9: Arkade ↔ RBTC on Rootstock: `createSwap` accepts RBTC as a source for
  Arkade, `fundSwap` funds the native lock in a single payable transaction,
  and the claim signs the native domain for an RBTC target. The tracker's
  EVM → Arkade leg now uses the swap's coordinator as the lock's sender and
  the zero asset word for a native lock.
- 101ebea: On-chain Bitcoin ↔ RBTC on Rootstock: `createSwap` accepts RBTC as a source
  for on-chain Bitcoin, `fundSwap` funds the native lock in a single payable
  transaction, and the claim signs the native domain for an RBTC target. The
  tracker's EVM → Bitcoin leg now uses the swap's coordinator as the lock's
  sender.
- c8b3c05: RBTC → Lightning swaps: `fundSwap` funds a native Rootstock lock with a single
  payable transaction, refunds pick the native contract calls, and `createSwap`
  accepts RBTC on chain `"30"` as a source. `EvmSigner` adapters should pass
  `value` through on `sendTransaction` and `call`, and may implement
  `getBalance`.
- 630b4e9: Add `client.claimEvmWithSigner(swapId, signer)`: claim an Arbitrum-hub BTC → EVM swap from the user's own wallet, which pays the gas. It publishes the same signed `redeemAndExecute` calldata as the sponsored UserOp path, so it works without AA / paymaster config and serves as the manual fallback when the automatic claim fails. `buildRedeemAndExecuteTx` and `claimViaSigner` are exported for custom integrations.

### Patch Changes

- 28e51fb: Target backend 0.3.16 in the x-satora-server-version header.
- fad103c: A gasless claim that is already on its way resolves instead of throwing.

  The server now answers a repeated claim, or one that arrives while it is
  relaying, with `200`, the swap's post-claim status and the claim transaction
  hash (empty until the relay is broadcast). The claim helpers pass that through
  as a successful claim, and still accept the `400` older servers answer with,
  so the auto-claim worker and the frontend stop retrying while the relay
  confirms.

- f79264e: Rootstock testnet (chain `"31"`). The EVM tracker knows its block cadence,
  public RPC and confirmation depth, so swaps a testnet daemon reports on
  chain `"31"` are tracked like mainnet Rootstock ones.
- bf5ea77: Claim EVM → Bitcoin and EVM → Arkade swaps whose DEX lock came in short.

  When the DEX returned less than quoted, the server accepted the lock and paid
  out a correspondingly lower amount, but chain-verified tracking marked the
  swap invalid and never claimed. It now accepts any payout the server's
  slippage rule allows.

## 1.5.0

### Minor Changes

- 4a99ca0: Let the Bitcoin confirmation depth change after the client is built.

  `setBitcoinMinConfirmations` updates the depth a server-funded Bitcoin HTLC
  needs before it reads as claimable, so callers that cache a single client can
  change it without rebuilding and orphaning tracking already in flight. It
  applies to every swap the client tracks, including ones already in flight,
  and at a depth above 1 the at-risk chain re-read runs every 15s instead of 60s
  so the claim follows the satisfying block promptly.

  Only the chain monitors read a depth, so it needs `withChainVerifiedTracking`;
  setting one without it now warns rather than silently doing nothing. A depth
  that is not a whole block count is rejected instead of stalling every read.

- d84290b: Lightning → RBTC on Rootstock.

  Follows `@lendasat/lendaswap-sdk-pure`: chain `"30"`, the native-lock claim,
  and RBTC kept on Rootstock instead of the Arbitrum hub. Chain-verified
  tracking knows Rootstock's block time (~30s) and its public HTTP RPC
  (`https://public-node.rsk.co`); override it with `withEvmRpcUrls` as for the
  other chains.

### Patch Changes

- 1519eac: Target backend 0.3.15 in the x-satora-server-version header.
- d299719: Chain-verified tracking stops recommending a claim the server already relayed.

  A server status hint of `clientredeeming`, `clientredeemed` or `serverredeemed`
  now overrides a chain view that still says "claim now". The chain still wins
  once it moves, and a refund observed on chain is never overridden. Without
  this, a chain the tracker cannot read (the public Rootstock RPCs serve no
  `eth_getLogs`) kept the frontend re-submitting claims.

  EVM log reads are chunked on chains whose providers cap the block span of
  `eth_getLogs` (Rootstock: 2000 blocks on rpc.rootstock.io).

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
