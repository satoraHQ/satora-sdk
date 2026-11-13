---
"@lendasat/lendaswap-sdk-pure": patch
---

`signEscrowArkTx`, `signEscrowCheckpoints`, `signEscrowDelegate` and
`getArkTxid` accept the PSBTs the ark-escrow gem builds again. ark-core records
the spend script as `witnessScript` on taproot inputs, and `@scure/btc-signer`
2.4 rejects that with `checkScript: witnessScript without P2WSH`, so every
escrow release, refund and refresh failed at the signing step. The helpers now
drop that field from taproot inputs and parse the result with all checks on.
