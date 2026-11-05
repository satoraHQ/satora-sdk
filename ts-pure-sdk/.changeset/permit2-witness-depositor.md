---
"@lendasat/lendaswap-sdk-pure": patch
---

Permit2 funding on HTLCErc20 v7 / HTLCNative v2 signs the witness with `refundAddress = depositor`, as coordinator v5 binds it; older deployments keep the coordinator. The funding endpoints now report `htlc_kind` and `htlc_version`, and `getPermit2FundingParamsUnsigned` takes the funding wallet's address.
