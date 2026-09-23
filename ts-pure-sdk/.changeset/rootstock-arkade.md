---
"@lendasat/lendaswap-sdk-pure": minor
---

Arkade ↔ RBTC on Rootstock. `createSwap` routes RBTC (chain `"30"`, the
zero-address token) → Arkade to the EVM → Arkade path, and `fundSwap` funds
that swap's native lock with one payable transaction, keyed on the
`evm_htlc_kind` the EVM → Arkade response now carries alongside its
`evm_coordinator_address`. The Arkade → EVM response carries `evm_htlc_kind`
too, so the gasless claim signs the `HTLCNative` domain for an RBTC target.
`NATIVE_TOKEN_ADDRESS` is exported.
