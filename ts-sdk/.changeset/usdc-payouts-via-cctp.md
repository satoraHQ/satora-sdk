---
"@satora/swap": minor
---

USDC payouts on Ethereum and Polygon settle on Arbitrum and bridge over CCTP. `getQuote`, `composeQuote` and `createSwap` route a swap whose target is native USDC on either chain the way they route USDC on Base: the HTLC locks tBTC on Arbitrum, the claim swaps to Arbitrum USDC, and Circle's Forwarding Service mints it on the destination. The quote's `bridge_fee` carries the CCTP fee. Swaps funded from USDC on these chains, and every other token on them, still run on the chain directly.

New helpers: `isCctpRoutedUsdcTarget(chain, token)` and `isBridgedTarget(chain, token)`, which is true for any target that settles on Arbitrum and bridges.
