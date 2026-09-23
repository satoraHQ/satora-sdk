---
"@lendasat/lendaswap-sdk-pure": minor
---

On-chain Bitcoin ↔ RBTC on Rootstock. `createSwap` routes RBTC (chain
`"30"`, the zero-address token) → Bitcoin to the EVM → Bitcoin path, and
`fundSwap` funds that swap's native lock with one payable transaction, keyed
on the `evm_htlc_kind` the EVM → Bitcoin response now carries alongside its
`evm_coordinator_address`. The Bitcoin → EVM response carries `evm_htlc_kind`
too, so the gasless claim signs the `HTLCNative` domain for an RBTC target.
