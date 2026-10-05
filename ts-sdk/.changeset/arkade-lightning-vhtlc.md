---
"@satora/swap": patch
---

Arkade to Lightning creation, and recovery of a swap not stored before, check
`arkade_vhtlc_address` against a VHTLC rebuilt from the invoice's payment hash
and the swap's refund key (see the legacy SDK changeset).
