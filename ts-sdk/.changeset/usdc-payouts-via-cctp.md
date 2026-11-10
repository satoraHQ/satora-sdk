---
"@satora/swap": minor
---

USDC payouts on Ethereum and Polygon settle on Arbitrum and bridge over CCTP. `getQuote`, `composeQuote` and `createSwap` route a swap whose target is native USDC on either chain the way they route USDC on Base: the HTLC locks tBTC on Arbitrum, the claim swaps to Arbitrum USDC, and Circle's Forwarding Service mints it on the destination. The quote's `bridge_fee` carries the CCTP fee. Swaps funded from USDC on these chains, and every other token on them, still run on the chain directly.

New helpers: `isCctpRoutedUsdcTarget(chain, token)` and `isBridgedTarget(chain, token)`, which is true for any target that settles on Arbitrum and bridges.

`createSwap` now refuses a source-pinned CCTP payout whose quoted net output after the bridge fee is not positive. Before, such a swap could be funded and then never claimed, because only the claim checks the fee against the output. Circle's forwarding fee to Ethereum is over a dollar.
