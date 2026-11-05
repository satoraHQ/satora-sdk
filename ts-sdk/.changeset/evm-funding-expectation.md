---
"@satora/swap": patch
---

Delegates `getEvmFundingExpectation(swapId, pins?)`: the hash lock and funding terms the client fixed for an EVM-sourced swap, to pass as `expected` when building CCTP-inbound calldata yourself (`buildCctpInboundBatch` now requires it).
