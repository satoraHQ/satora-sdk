---
"@satora/swap": patch
---

Chain-verified tracking stops recommending a claim the server already relayed.

A server status hint of `clientredeeming`, `clientredeemed` or `serverredeemed`
now overrides a chain view that still says "claim now". The chain still wins
once it moves, and a refund observed on chain is never overridden. Without
this, a chain the tracker cannot read (the public Rootstock RPCs serve no
`eth_getLogs`) kept the frontend re-submitting claims.

EVM log reads are chunked on chains whose providers cap the block span of
`eth_getLogs` (Rootstock: 2000 blocks on rpc.rootstock.io).
