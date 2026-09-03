---
"@lendasat/lendaswap-sdk-pure": minor
---

Add EVM → Lightning swaps: `createEvmToLightningSwap` (invoice, lightning address or LNURL destination, funded from USDC/WBTC/tBTC on Polygon, Ethereum or Arbitrum), `createSwap` routing to it for Lightning destinations, and refund / Permit2 / gasless support for the new direction.

`getLightningSendQuote` now accepts EVM sources via `sourceChain` + `sourceToken`. `sourceAmount` (the source token's smallest unit, as a string in the result) replaces `sourceAmountSats`, which stays as a deprecated alias: accepted as a param, and set on Arkade-source results.
