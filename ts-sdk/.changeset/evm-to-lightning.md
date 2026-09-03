---
"@satora/swap": minor
---

Add EVM → Lightning swaps: `createEvmToLightningSwap` (invoice, lightning address or LNURL destination, funded from USDC/WBTC/tBTC on Polygon, Ethereum or Arbitrum) with tracking, and `createSwap` routing to it for Lightning destinations.

`getLightningSendQuote` now accepts EVM sources via `sourceChain` + `sourceToken`. `sourceAmount` (the source token's smallest unit, as a string in the result) replaces `sourceAmountSats`, which stays as a deprecated alias: accepted as a param, and set on Arkade-source results.

Tracking: a swap paying out over Lightning no longer surfaces a refund while the server's payment is in flight (`serverfunded`); the derived action is a wait, with the refund listed but blocked until the deposit's own timelock.

`fundSwap` reports the source amount the server quoted for the attempt through a new `onQuote` option (an EVM → Lightning swap is priced afresh on every funding) and refreshes the stored swap once the funding lands. A funding that would revert now throws `SimulationRevertError`, carrying the revert reason, instead of a plain `Error`.
