---
"@lendasat/lendaswap-sdk-pure": minor
---

Add `client.claimEvmWithSigner(swapId, signer)`: claim an Arbitrum-hub BTC → EVM swap from the user's own wallet, which pays the gas. It publishes the same signed `redeemAndExecute` calldata as the sponsored UserOp path, so it works without AA / paymaster config and serves as the manual fallback when the automatic claim fails. `buildRedeemAndExecuteTx` and `claimViaSigner` are exported for custom integrations.
