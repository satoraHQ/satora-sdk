---
"@lendasat/lendaswap-sdk-pure": minor
---

RBTC → Lightning: `fundSwap` funds a native lock (an EVM → Lightning swap whose
`evm_htlc_kind` is `native`) with one payable `executeAndCreate` on the native
coordinator, carrying the quoted amount as the transaction value; no token
approval, Permit2 signature or server calldata. `EvmSigner.sendTransaction` and
`call` take an optional `value`, and an optional `getBalance` lets the SDK
refuse an underfunded wallet before it signs. Refunds built from the funding
log pick the native `refundTo` / `refund` when the lock's asset word is zero.
`createSwap` and the Lightning-send quote accept RBTC on Rootstock (chain
`"30"`, the zero-address token) as a source (`isEvmSwapSource`).
