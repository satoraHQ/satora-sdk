---
"@lendasat/lendaswap-sdk-pure": minor
---

Claim RBTC through RIF Relay where the server publishes one. `getRifRelay(chainId)`
reads it from `/chain-config` (new optional `rif_relay` on each entry). On such a
chain an RBTC swap is created with the recipient's RIF smart wallet as its claim
address (`rifSmartWalletAddress(relay, recipient)`; the server rejects any other),
so only the recipient's wallet can claim it: `claimEvmWithSigner(swapId, signer)`
with that wallet relays `HTLCNative.redeem` from the smart wallet, deploying it
on the first claim. The relayer's fee is paid out of the claimed RBTC and the
rest lands at the recipient, which needs no gas. `claim()` and
`claimViaGasless()` throw `WalletClaimRequiredError` for these swaps.
