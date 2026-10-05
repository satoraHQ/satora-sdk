---
"@lendasat/lendaswap-sdk-pure": patch
---

Update the generated API docs for `/api/delegate/settle`: a `swap_id` sent with
a `preimage` now requires the intent to spend that swap's VHTLC, and the backend
records the claim for Lightning to Arkade swaps as well.
