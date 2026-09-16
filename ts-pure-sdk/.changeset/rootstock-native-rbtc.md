---
"@lendasat/lendaswap-sdk-pure": minor
---

Lightning → RBTC on Rootstock.

`Chain` and `WireChain` gain `"30"` (Rootstock). RBTC is the chain's own coin,
locked in `HTLCNative` rather than an ERC-20 HTLC, and is addressed by the zero
token address. The generated types carry `evm_htlc_kind` (`"erc20"` or
`"native"`) on the responses of the directions that can lock it.

`claim` on a swap whose lock is native signs `HTLCNative`'s `Redeem` instead of
`HTLCErc20`'s: `buildNativeRedeemDigest` (exported) builds the token-less
digest under the `HTLCNative` domain, and the relayed claim sweeps the coin to
the destination with the full locked amount as the floor and no calls. There
is no DEX leg on this route, so the target amount equals the lock.

Rootstock is also a USDT0 bridge destination, so the chain alone does not say
whether a target is bridged: `isNativeLockTarget(chain, token)` (exported) is
true for RBTC on chain 30, and `getQuote` / `createSwap` keep that target on
Rootstock instead of remapping it to the Arbitrum hub. USDT0 on Rootstock
still bridges. `NATIVE_TOKEN_ADDRESS` is exported.
