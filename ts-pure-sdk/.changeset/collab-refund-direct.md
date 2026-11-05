---
"@lendasat/lendaswap-sdk-pure": minor
---

Collaborative refunds on HTLCErc20 v7 / HTLCNative v2 go through the depositor. `getCollabRefundEvmParams` reports `refundPath` (`direct`, `coordinator`, `unavailable`) and the HTLC. On `direct`, `collabRefundEvmWithSigner` fetches the server's `Refund` signature for the wallet (new `getCollabRefundEvmSignature`) and sends `refundBySig` from it; `collabRefundEvmSwap` does the same from the SDK key as a sponsored UserOp on Arbitrum. The `coordinator` path is unchanged.
