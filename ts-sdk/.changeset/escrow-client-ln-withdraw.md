---
"@satora/escrow-client": major
---

Re-enable Lightning withdrawals on the rebuilt Arkade→Lightning swaps.

- `withdrawToLightning` / `withdraw` (Lightning destinations) create an
  Arkade→Lightning swap and fund its VHTLC from the payout wallet again,
  instead of throwing.
- `quoteLightningWithdrawal(sourceAmountSats, destination?)` quotes via
  `getQuote` (Arkade → Lightning); pass the destination to price the real
  Lightning send fee.
- Breaking: `SwapClient` now also picks `createArkadeToLightningSwap` and
  `getQuote` (the swap `Client` already has both), and
  `LightningWithdrawalUnavailableError` is removed.
