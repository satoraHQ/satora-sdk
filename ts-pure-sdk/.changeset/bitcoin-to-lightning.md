---
"@lendasat/lendaswap-sdk-pure": minor
---

Bitcoin (on-chain) ↔ Lightning swaps.

`createBitcoinToLightningSwap` locks BTC in an on-chain HTLC against a
BOLT11 invoice, lightning address or LNURL; the server pays the invoice
after one confirmation and claims the HTLC with the revealed preimage.
`refundSwap` sweeps the HTLC after its locktime like the other on-chain
directions, and `getLightningSendQuote` / `getQuote` with a concrete
destination price the route exactly (`sourceChain: "Bitcoin"`).

`createLightningToBitcoinSwap` pays a hold invoice against an on-chain
HTLC the server funds; `claim()` sweeps it to the swap's target address,
which settles the invoice.

`createSwap` routes both directions.

On-chain claims (`evm_to_bitcoin` and `lightning_to_bitcoin`) now pay the
explorer's two-block fee estimate by default instead of a flat 2 sat/vB,
which stays the floor; `feeRateSatPerVb` still overrides it.
