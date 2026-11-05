---
"@satora/swap": patch
---

`refundEvmSponsored` refunds an HTLCErc20 v7 lock with the smart account's own `refund`, which the paymaster sponsors, instead of `refundFor` (see `@lendasat/lendaswap-sdk-pure`).
