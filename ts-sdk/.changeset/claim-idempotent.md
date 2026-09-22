---
"@satora/swap": patch
---

A gasless claim that is already on its way resolves instead of throwing.

The server now answers a repeated claim, or one that arrives while it is
relaying, with `200`, the swap's post-claim status and the claim transaction
hash (empty until the relay is broadcast). The claim helpers pass that through
as a successful claim, and still accept the `400` older servers answer with,
so the auto-claim worker and the frontend stop retrying while the relay
confirms.
