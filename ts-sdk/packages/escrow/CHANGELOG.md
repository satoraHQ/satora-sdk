# @satora/escrow

## 0.1.0

### Minor Changes

- f61deb6: Bump `@arkade-os/sdk` to `^0.5.1` (pulls in the Arkade claim fix from `@lendasat/lendaswap-sdk-pure`). The `@arkade-os/sdk` peer range moves from `^0.4.66` to `^0.5.1`, which is breaking for consumers still on 0.4.

  `@satora/escrow` / `@satora/escrow-client`: `escrowArkConfigFromInfo` now takes the 0.5 `ArkadeInfo`, `dispose()` returns a `Promise<void>`, and wallet roles resolve from `PathContext.walletDescriptor` (bare x-only hex or `tr(<key>)`).

## 0.0.6

### Patch Changes

- 0f7f94f: Target backend 0.3.10 in the x-satora-server-version header and align the
  Arkade SDK dependency on 0.4.66.

## 0.0.5

### Patch Changes

- 9f4d595: Add package READMEs for `@satora/escrow` and `@satora/swap`.
- bbba274: Add package README (published to npm).

## 0.0.5-rc.0

### Patch Changes

- 9f4d595: Add package READMEs for `@satora/escrow` and `@satora/swap`.
- bbba274: Add package README (published to npm).
