---
"@satora/swap": patch
"@satora/escrow": patch
---

Escrow signing accepts the PSBTs the ark-escrow gem builds again.
`signEscrowArkTx`, `signEscrowCheckpoints` (and in `@satora/swap` also
`signEscrowDelegate` and `getArkTxid`) threw
`checkScript: witnessScript without P2WSH`, because ark-core records the spend
script as `witnessScript` on taproot inputs and `@scure/btc-signer` 2.4 rejects
that. The helpers now drop that field from taproot inputs and parse the result
with all checks on. The signed PSBT they return no longer carries it.
