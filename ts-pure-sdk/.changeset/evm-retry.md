---
"@lendasat/lendaswap-sdk-pure": minor
---

Swap a refunded EVM-sourced swap again instead of leaving the refund behind.
`refundEvmSponsored(swapId)` refunds a swap whose depositor is this client's
key (created `gasless`, incl. CCTP-inbound) as a paymaster-sponsored UserOp on
Arbitrum, no wallet or gas needed. `continueRefundedEvmSwap` then swaps the
refund again: its `signer` is now optional (defaults to this client's key), it
takes a `target` (`"bitcoin" | "arkade" | "lightning"`, defaulting to the old
swap's), and it can pay out to a Lightning invoice, Lightning address or LNURL.
`getRefundedEvmSwapContinuation` accepts Lightning swaps and an optional signer.
