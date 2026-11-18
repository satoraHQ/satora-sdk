---
"@satora/escrow-client": patch
---

`withdrawToLightning` accepts an optional `sourceAmountSats` for a BOLT11 invoice: a fixed lock that locks exactly that amount from the payout (quote with `quoteLightningWithdrawal(sourceSats)`, invoice `recipientSats`, then withdraw with `sourceAmountSats: sourceSats`). `toLightningDestination` gains a matching third argument. Passing `sourceAmountSats` with an LNURL / Lightning address throws.
