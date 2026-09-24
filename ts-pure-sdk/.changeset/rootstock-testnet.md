---
"@lendasat/lendaswap-sdk-pure": patch
---

Rootstock testnet (chain `"31"`). A testnet daemon reports Rootstock under
its own id, and the SDK now keeps it as `"31"` instead of collapsing it into
mainnet's `"30"`. It is typed on `Chain` and the Lightning send source
chains, and `isNativeLockChain` recognises it, so RBTC swaps route and fund
the same way on testnet.
