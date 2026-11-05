---
"@lendasat/lendaswap-sdk-pure": minor
---

Declare EVM HTLC protocol epoch 2 (`x-satora-evm-erc20-htlc-version: 2`, `x-satora-evm-native-htlc-version: 2`): new swaps are created on HTLCErc20 v7 / HTLCCoordinator v5 / HTLCNative v2. Adds `ProtocolEpoch.V2`. Swaps already on v6 keep settling as before; released SDKs (epoch 1) keep being served the v6 deployment.
