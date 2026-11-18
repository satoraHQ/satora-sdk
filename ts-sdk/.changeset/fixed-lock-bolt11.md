---
"@satora/swap": patch
---

Arkade/Bitcoin → Lightning: allow `sourceAmountSats` together with `lightningInvoice` (and `sourceAmount` with a BOLT11 destination in `createSwap` / `getLightningSendQuote`) for a fixed lock amount. The created swap's `source_amount` is checked to equal the requested lock before it is stored.
