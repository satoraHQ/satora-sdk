---
"@satora/swap": minor
---

Retry an expired gasless EVM-to-Bitcoin swap from its deposit. `createSwap` takes `fundFromSwapId`: the new swap is funded from the deposit account of an earlier swap of this client that the server expired before anything was locked, so a deposit that arrived too late counts towards the new swap. The earlier swap must be gasless, on the same chain and token, funded from an SDK-derived account, and have no funding attempt on record. Each expired swap can be retried once: it records the retry as `supersededBy`. Seed-only recovery links a retry back to the account it reuses. Sponsored gasless funding now refuses a swap that is not pending.
