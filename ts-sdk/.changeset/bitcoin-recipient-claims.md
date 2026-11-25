---
"@lendasat/lendaswap-sdk-pure": minor
"@satora/swap": minor
---

Add multi-recipient Bitcoin claim primitives and persist recipient commitments in swap storage. Claims preserve recipient order and committed minimums, distribute surplus equally among recipients, and signal replaceability. The committed fee is a minimum: claims raise it out of surplus to meet the fee rate, so a retry at a higher rate bumps the fee, and refuse before broadcast if the surplus or the 100,000-sat fee cap cannot cover it. Recipients never drop below their committed amounts. Existing single-address claims remain unchanged.
