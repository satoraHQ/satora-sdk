---
"@satora/swap": patch
---

Backend behaviour for fixed-lock Lightning sends (`lightningInvoice` with `sourceAmountSats`), no API shape change:

- When the lock falls a few sats short of what the invoice costs (the actual Lightning fee for that invoice came in above the `/quote` estimate), the backend may now accept the swap instead of returning a 400, depending on the route's configuration. The created swap then has a lower `fee_sats` (and, at most by the allowed loss, a lower `network_fee_sats`); `source_amount` still equals the requested lock and the invoice is paid in full.
- When it is still refused, the 400 now reads `... request a smaller invoice (at most N sats)` instead of `(about N sats)`. An invoice of at most N sats fits the same lock unless the provider's fee quote rises in between, so one retry is enough.
- A fixed lock smaller than the invoice amount itself is always refused.
