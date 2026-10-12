---
"@satora/swap": minor
---

RBTC claims through RIF Relay: `getRifRelay(chainId)` and the RIF branch of
`claimEvmWithSigner` (see the legacy SDK changeset). The auto-claim worker hands
a swap that needs the user's wallet to `onActionRequired` once instead of
retrying it.
