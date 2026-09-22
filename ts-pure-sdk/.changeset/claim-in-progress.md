---
"@lendasat/lendaswap-sdk-pure": patch
---

`claimViaGasless` no longer throws when the server has already relayed, or is
already relaying, a claim for the same swap: the result carries the swap's
post-claim status (and the claim transaction hash when the server has it), so
the auto-claim worker and the frontend stop retrying while the relay confirms.
Covers both the current server's idempotent `200` and the `400` older servers
answer with.
