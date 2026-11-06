---
"@satora/swap": patch
---

Re-read the server's HTLC 1, 2, 4 and 8 seconds after a status hint says it is funded while the chain read does not show it yet. The public RPCs trail the server's node by a few blocks, so the first read often missed the lock and the auto-claim waited for the next at-risk reconcile, up to a minute.
