---
"@lendasat/lendaswap-sdk-pure": patch
---

Claims pay out to the target the swap was created with, never to an address
a server answer names. Every creation with a payout (to EVM, Bitcoin or
Arkade) refuses a response whose `target_evm_address`, `target_btc_address` or
`target_arkade_address` is not the `targetAddress` sent, and to EVM creation
also one whose `client_evm_address` is not the claiming address sent, with a
`SwapVerificationError`; nothing is stored. The requested target is stored as
`StoredSwap.targetAddress` and kept by recovery. `claim`,
`claimEvmWithSigner`, the on-chain Bitcoin claim and the Tron bridge
recipient take the destination from the caller, then that target, then the
stored copy's target field for the swap's direction; a fresh server copy is
no longer read for it, and the Arkade-target `claim` no longer fetches one.
`getSwap(id, { updateStorage: true })` and `recoverSwaps` also refuse a server
copy that changes a stored swap's payout addresses, `client_evm_address` or
`bridge_target_chain`, for every direction including EVM-sourced ones (for
those only these fields are kept from the stored copy).
`SqliteSwapStorage` now saves `targetAddress`, `bridgeRecipient` and
`bridgeRecipientWallet` (adding their columns to an existing database), which
it dropped before, so a bare `claim` of a Solana bridge swap stored there no
longer needs the recipient passed again.
