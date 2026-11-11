---
"@lendasat/lendaswap-sdk-pure": patch
---

Bitcoin to Arkade, Bitcoin to EVM and Arkade to EVM creation rebuild the lock
the user funds from the swap's preimage hash and refund key, the same way a
refund does, and refuse the swap with a `SwapVerificationError` unless
`btc_htlc_address` (Bitcoin to Arkade, Bitcoin to EVM) or `btc_vhtlc_address`
(Arkade to EVM) matches, so a server cannot have the user fund an address it
alone can spend. The HTLC's hash lock must be the HASH160 of the swap's
preimage hash, and a script version this client cannot refund fails.
`recoverSwaps` makes the same checks before it stores a swap it did not hold
before, and leaves out one that fails.
