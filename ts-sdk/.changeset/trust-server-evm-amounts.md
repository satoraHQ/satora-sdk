---
"@satora/swap": patch
---

Claim EVM → Bitcoin and EVM → Arkade swaps whose DEX lock came in short.

When the DEX returned less than quoted, the server accepted the lock and paid
out a correspondingly lower amount, but chain-verified tracking marked the
swap invalid and never claimed. It now accepts any payout the server's
slippage rule allows.
