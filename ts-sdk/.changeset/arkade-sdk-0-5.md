---
"@satora/swap": patch
"@satora/escrow": minor
"@satora/escrow-client": major
---

Bump `@arkade-os/sdk` to `^0.5.1` (pulls in the Arkade claim fix from `@lendasat/lendaswap-sdk-pure`). The `@arkade-os/sdk` peer range moves from `^0.4.66` to `^0.5.1`, which is breaking for consumers still on 0.4.

`@satora/escrow` / `@satora/escrow-client`: `escrowArkConfigFromInfo` now takes the 0.5 `ArkadeInfo`, `dispose()` returns a `Promise<void>`, and wallet roles resolve from `PathContext.walletDescriptor` (bare x-only hex or `tr(<key>)`).
