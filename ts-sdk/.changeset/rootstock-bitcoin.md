---
"@satora/swap": minor
---

On-chain Bitcoin ↔ RBTC on Rootstock: `createSwap` accepts RBTC as a source
for on-chain Bitcoin, `fundSwap` funds the native lock in a single payable
transaction, and the claim signs the native domain for an RBTC target. The
tracker's EVM → Bitcoin leg now uses the swap's coordinator as the lock's
sender.
