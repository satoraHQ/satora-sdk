---
"@satora/swap": patch
"@satora/escrow": patch
---

Arkade claims, refunds, collaborative refunds and escrow releases now submit in one call (one-shot submit): checkpoints are signed up front and sent with the ark tx. Older Arkade servers still get a separate `finalizeTx`. `submitAndFinalizeEscrowRelease` now matches the server's checkpoints by txid instead of position.
