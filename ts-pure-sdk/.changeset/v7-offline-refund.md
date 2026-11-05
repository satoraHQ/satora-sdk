---
"@lendasat/lendaswap-sdk-pure": patch
---

`refundSwap` with a signer builds the refund of a swap on HTLCErc20 v7 / HTLCNative v2 from the funding tx again. On those deployments the depositor is the lock's sender, so the chain read now accepts a `SwapCreated` from the signer or one of the swap's expected depositors (not the coordinator), and the refund is the HTLC's permissionless `refundFor`, paying the depositor, byte for byte what the server builds. The depositor is no longer recorded as `StoredSwap.evmCoordinatorAddress`. Older deployments keep the coordinator's `refundTo`.
