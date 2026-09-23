---
"@satora/swap": minor
---

Arkade ↔ RBTC on Rootstock: `createSwap` accepts RBTC as a source for
Arkade, `fundSwap` funds the native lock in a single payable transaction,
and the claim signs the native domain for an RBTC target. The tracker's
EVM → Arkade leg now uses the swap's coordinator as the lock's sender and
the zero asset word for a native lock.
