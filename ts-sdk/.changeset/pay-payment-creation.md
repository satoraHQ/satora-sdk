---
"@lendasat/lendaswap-sdk-pure": minor
"@satora/swap": minor
---

Create EVM-to-Bitcoin payments to several recipients. `createSwap` takes a `bitcoinPayout` payment request with nominal amounts, the claim fee and an optional payout tolerance of up to 1%. The SDK commits each recipient to its floor (nominal less the tolerance, never below dust) and asks the server for a fee-inclusive minimum payout; the server reserves DEX headroom before funding. Claims split any surplus in proportion to the commitments, so a payment funded at its nominal total pays roughly the nominal amounts. Payment claims also refuse to reveal the preimage too close to the refund expiry, use the explorer's relay minimum as a floor for the fee rate, and record a submitted claim. Payment commitments, display metadata and depositor key references persist in SQLite, and depositor keys resolve consistently for funding, refunds and continuations.
