---
"@lendasat/lendaswap-sdk-pure": patch
---

Arkade to Lightning creation rebuilds the VHTLC from the invoice's payment
hash, the swap's refund key and the server's keys and timelocks, the same way
a refund does, and refuses the swap with a `SwapVerificationError` unless
`arkade_vhtlc_address` matches, so a server cannot have the user fund a VHTLC
on a hash it knows. Bitcoin to Lightning creation does the same for the
on-chain HTLC: `btc_hash_lock` must be the HASH160 of the invoice's payment
hash and `btc_htlc_address` must rebuild from it, the swap's refund key and
the server's key and locktime. `recoverSwaps` makes the same checks, with the
refund key derived at the server's index, before it stores a swap it did not
hold before, and leaves out one that fails. A swap on a script version this
client cannot refund fails, including one created before strict scripts.
