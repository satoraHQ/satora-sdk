# @satora/swap

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
