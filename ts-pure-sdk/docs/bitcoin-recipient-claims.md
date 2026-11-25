# Multi-recipient Bitcoin claim primitive

`buildOnchainClaimTransaction` accepts an optional `bitcoinPayout`:

```ts
bitcoinPayout: {
  recipients: [
    { address: firstAddress, amountSats: "30000" },
    { address: secondAddress, amountSats: "68000" },
  ],
  feeSats: "500",
}
```

Amounts are integer satoshi strings. The primitive supports 1–100 standard
Bitcoin addresses on the claim's network, preserves order and duplicate
addresses, and requires at least 546 sats per recipient. The explicit fee is
1–100,000 sats; fractional sat/vB rates are supported when checking the signed
transaction's actual virtual size.

For a 100,000-sat HTLC this example pays 30,750 and 68,750 sats. After the
explicit fee and committed amounts, surplus is divided equally across recipient
outputs. Any leftover sats go to recipients in list order (one extra sat each).
There is no separate recovery/change output, even for a one-sat surplus.
Shortfalls reject rather than reducing the committed amounts.

Claims with recipient commitments signal RBF. A higher fee, whether from a
higher `feeSats` or raised to meet a fee rate, consumes previously distributed
surplus. For example, a 1,000-sat fee pays
30,500 and 68,500 sats. A 2,000-sat fee pays the original 30,000 and 68,000 sats;
a larger fee rejects. Replacement transactions must still satisfy the node's
replacement and relay rules. The original commitments remain unchanged in storage
and backups; a replacement recalculates surplus from those commitments.

`StoredSwap.bitcoinPayout` is local data: SQLite persists it, response refreshes
retain it, and full JSON exports of the stored swap include it. `Client.claim`
uses the stored commitments and refuses an explicit `destinationAddress` rather
than silently ignoring it. The committed `feeSats` is a minimum. Each claim
raises the fee out of surplus to meet the fee rate: the caller-supplied
`feeRateSatPerVb`, which must be above zero, otherwise the two-block estimate
with a 2 sat/vB floor. Calling `claim` again at a higher rate is therefore a
fee bump. Recipients never drop below their committed amounts, and the stored
commitment is not rewritten. If the surplus cannot cover the rate, or the fee
would exceed the 100,000-sat payout cap, the claim is refused before anything is
broadcast. A caller-supplied rate is trusted as given, so a low one can still
broadcast a slow claim; call `claim` again with a higher rate to replace it.

A complete recovery backup must contain the full stored swap, including its
recipient commitments, keys/preimage, funding response and recovery target.
A seed alone cannot reconstruct the recipient list. Omitting `bitcoinPayout`
from the low-level builder explicitly selects the existing single-destination
claim; this recovers funds to `destinationAddress` but does not pay recipients.
Do not silently switch to that mode when a payment claim fails.

This slice does not add recipient options to swap creation, new backend routes,
or automatic payment funding. Funding amount guarantees, pre-funding capability
checks and the Pay creation API are separate prerequisites before exposing Pay.
Existing swap creation and single-destination claims are unchanged.
