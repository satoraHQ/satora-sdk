---
"@satora/swap": minor
---

RBTC → Lightning swaps: `fundSwap` funds a native Rootstock lock with a single
payable transaction, refunds pick the native contract calls, and `createSwap`
accepts RBTC on chain `"30"` as a source. `EvmSigner` adapters should pass
`value` through on `sendTransaction` and `call`, and may implement
`getBalance`.
