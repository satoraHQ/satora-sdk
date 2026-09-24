---
"@lendasat/lendaswap-sdk-pure": minor
---

Inbound swaps (Bitcoin / Arkade / Lightning → EVM) are now created with the swap's own key as `claiming_address` instead of the wallet-level EVM address. A sponsored UserOp claim installs an EIP-7702 Kernel delegation on the claiming address; keeping that off the wallet-level address means gasless deposits (`fundSwapGasless`) keep signing Permit2 and EIP-2612 as a plain EOA, and the one-address / single-approval deposit UX is unchanged. Swaps created before this release still carry the wallet-level claim address and keep claiming with it; recovery re-derives per-swap keys by index as before.
