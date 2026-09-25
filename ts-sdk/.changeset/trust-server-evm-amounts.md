---
"@satora/swap": patch
---

Claim EVM → Bitcoin and EVM → Arkade swaps whose DEX lock came in short.

The coordinator locks whatever the DEX returns, so the lock can land below
`evm_expected_sats`. The server accepts that and lowers the payout by the
shortfall. Chain-verified tracking still compared both legs against the quote,
so it marked the swap invalid, showed "Waiting for refund" and never claimed
the BTC the server had already funded. For these two directions the tracker no
longer checks amounts: the server decides whether a short lock is acceptable
and what it pays. The legs stay pinned by hash lock, claim address, token and
script.
