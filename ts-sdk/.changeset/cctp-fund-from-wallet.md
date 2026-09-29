---
"@satora/swap": minor
---

`fundSwap` funds a CCTP-inbound swap (USDC from Base, Optimism, Avalanche, … to
Bitcoin) from a browser wallet, records the burn to resume after an
interruption, and `recoverCctpInboundSwap` recovers the USDC if settlement
never lands (see the legacy SDK changeset).
