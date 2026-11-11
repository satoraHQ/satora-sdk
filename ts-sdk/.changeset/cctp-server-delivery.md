---
"@satora/swap": patch
---

Track USDC payouts on Polygon and Ethereum that the Satora server delivers itself. For these the server now calls `receiveMessage` on the destination chain instead of Circle's Forwarding Service, so the quote's `bridge_fee` carries only Circle's fast-transfer fee and no forwarding fee. `trackCctpMessage` completes such a transfer on its destination mint (Circle's API reports no forward for it) and returns that transaction as `forwardTxHash`. Before, it kept reporting `FORWARDING` until it timed out.

The CCTP `bridge_rate` of a quote has a new `self_relayed` flag, and a claimed swap names the destination transaction in `bridge_target_txid` once the server has minted it.

Claims that fetch their calldata from `/dex-calldata` now name the swap (`swap_id`). The server only delivers a bridged payout itself for a swap it knows bridges to that chain, and only above the minimum payout size it is configured with; anything else keeps Circle's Forwarding Service and its fee.
