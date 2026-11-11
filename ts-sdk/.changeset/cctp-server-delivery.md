---
"@satora/swap": patch
---

Track USDC payouts on Polygon and Ethereum that the Satora server delivers itself. For these the server now calls `receiveMessage` on the destination chain instead of Circle's Forwarding Service, so the quote's `bridge_fee` carries only Circle's fast-transfer fee and no forwarding fee. `trackCctpMessage` completes such a transfer on its destination mint (Circle's API reports no forward for it) and returns that transaction as `forwardTxHash`. Before, it kept reporting `FORWARDING` until it timed out.

The CCTP `bridge_rate` of a quote has a new `self_relayed` flag, and a claimed swap names the destination transaction in `bridge_target_txid` once the server has minted it.
