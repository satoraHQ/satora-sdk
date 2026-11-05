---
"@lendasat/lendaswap-sdk-pure": patch
---

`refundEvmSponsored` refunds an HTLCErc20 v7 / HTLCNative v2 lock with the smart account's own `refund` instead of the server's permissionless `refundFor`, which the paymaster does not sponsor. New helper `refundForAsOwnRefund`.
