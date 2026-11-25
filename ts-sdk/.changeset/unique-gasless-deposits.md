---
"@satora/swap": minor
---

Use per-swap deposit addresses for new gasless EVM swaps. Select matching keys for Permit2 funding, refunds, CCTP settlement and recovery while retaining legacy shared-address support. Read the deposit address from the swap response or getSwapDepositorKey(swapId); getEvmAddress and getEvmDepositorKey refer only to the legacy wallet-level account. Refund continuations explicitly reuse their existing funded account.
