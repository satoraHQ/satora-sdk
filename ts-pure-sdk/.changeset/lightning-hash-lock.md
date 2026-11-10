---
"@lendasat/lendaswap-sdk-pure": patch
---

Check that a Lightning invoice pays the swap's hash lock. Lightning to Arkade,
Lightning to EVM and Lightning to Bitcoin creation decode the server's hold invoice and refuse it
with a `SwapVerificationError` unless it pays the hash of the swap's own
secret; nothing is stored. Arkade to Lightning and Bitcoin to Lightning
creation now make the checks
EVM to Lightning creation already made: `hash_lock` must be the payment hash
of the supplied `lightningInvoice`, or of the invoice the server resolved,
which must ask for the quoted payout. All three also refuse a
`client_lightning_invoice` other than the supplied invoice, and throw
`SwapVerificationError` for all of these. A malformed `lightningInvoice` to
Arkade or Bitcoin to Lightning creation is refused before any request. `getSwap(id, { updateStorage: true })` and `recoverSwaps` no longer replace a stored swap
with a server copy whose direction, hash locks, lock addresses or invoice
payment hashes differ, or whose invoice or hash lock fails the checks creation
makes: `getSwap` throws `SwapVerificationError` and recovery keeps the stored
copy, logging a warning. EVM-sourced swaps are not compared, since their
funding uses what creation recorded. Recovery also keeps a stored swap's keys
and preimage when the server's index would derive other ones, and stores a
swap it did not hold before only if it passes the same invoice and hash lock
checks, against the secret derived at the server's index. A swap that fails is
left out, with a warning. `invoicePaymentHash` reads an invoice the way LDK
does: it refuses a second payment hash or a truncated field, and takes the
first 256 bits of a payment hash whose padding bits are set.
