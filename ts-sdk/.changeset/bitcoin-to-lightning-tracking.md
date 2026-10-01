---
"@satora/swap": minor
---

Track Bitcoin (on-chain) ↔ Lightning swaps: the client-funded BTC HTLC of
Bitcoin → Lightning is observed like the Bitcoin → EVM one, the
server-funded HTLC of Lightning → Bitcoin like the EVM → Bitcoin one; the
Lightning legs have no chain leg.

On mainnet the tracker stops recommending a claim of a Lightning → Bitcoin
HTLC 30 minutes before its refund locktime, so a slow claim cannot reveal the
preimage to a racing refund.
