---
"@lendasat/lendaswap-sdk-pure": minor
"@satora/swap": minor
---

Add multi-recipient Bitcoin claim primitives and persist recipient commitments in swap storage. Claims preserve recipient order and committed minimums, distribute surplus equally among recipients, and signal replaceability. Explicit fee increases can spend surplus without reducing recipients below their committed amounts. Existing single-address claims remain unchanged.
