---
"@lendasat/lendaswap-sdk-pure": minor
---

HTLC-level `refundBySig` and `rehash` encoders for HTLCErc20 v7 / HTLCNative v2 (`encodeHtlcErc20RefundBySigCallData`, `encodeHtlcNativeRefundBySigCallData`, `encodeHtlcErc20RehashCallData`, `encodeHtlcNativeRehashCallData`). Both are sent by the depositor with the server's signature from `/collab-refund-evm/signature` and the new `/rehash-evm` endpoint; the regenerated API types carry both. `encodeRefundAndExecute` is removed: the coordinator no longer has that function and nothing called it.
