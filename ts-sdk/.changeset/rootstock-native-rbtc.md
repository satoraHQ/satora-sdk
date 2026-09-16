---
"@satora/swap": minor
---

Lightning → RBTC on Rootstock.

Follows `@lendasat/lendaswap-sdk-pure`: chain `"30"`, the native-lock claim,
and RBTC kept on Rootstock instead of the Arbitrum hub. Chain-verified
tracking knows Rootstock's block time (~30s) and its public HTTP RPC
(`https://public-node.rsk.co`); override it with `withEvmRpcUrls` as for the
other chains.
