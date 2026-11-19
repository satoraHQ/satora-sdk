---
"@satora/swap": patch
---

Fix resuming pending Arkade claims on Taproot checkpoints. Claim retries verify the requested recipient before finalizing and report errors from the resume attempt.
