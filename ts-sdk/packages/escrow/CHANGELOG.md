# @satora/escrow

## 0.1.3

### Patch Changes

- 3458cbf: Escrow signing accepts the PSBTs the ark-escrow gem builds again.
  `signEscrowArkTx`, `signEscrowCheckpoints` (and in `@satora/swap` also
  `signEscrowDelegate` and `getArkTxid`) threw
  `checkScript: witnessScript without P2WSH`, because ark-core records the spend
  script as `witnessScript` on taproot inputs and `@scure/btc-signer` 2.4 rejects
  that. The helpers now drop that field from taproot inputs and parse the result
  with all checks on. The signed PSBT they return no longer carries it.

## 0.1.2

### Patch Changes

- 800aa2f: Arkade claims, refunds, collaborative refunds and escrow releases now submit in one call (one-shot submit): checkpoints are signed up front and sent with the ark tx. Older Arkade servers still get a separate `finalizeTx`. `submitAndFinalizeEscrowRelease` now matches the server's checkpoints by txid instead of position.
- 42cc809: Use the current `@arkade-os/sdk` names (`OperatorProvider`, `RestOperatorProvider`, `Proof`, `SignedProof`, `OperatorInfo`) instead of their deprecated aliases. Type-only; no behaviour change.
- e926b23: Bump `@arkade-os/sdk` to 0.5.5.

## 0.1.1

### Patch Changes

- bc12fe9: The published package.json now records the `satoraHQ/swap` repository and directory. No code change.

## 0.1.0

### Minor Changes

- f61deb6: Bump `@arkade-os/sdk` to `^0.5.1` (pulls in the Arkade claim fix from `@lendasat/lendaswap-sdk-pure`). The `@arkade-os/sdk` peer range moves from `^0.4.66` to `^0.5.1`, which is breaking for consumers still on 0.4.

  `@satora/escrow` / `@satora/escrow-client`: `escrowArkConfigFromInfo` now takes the 0.5 `ArkadeInfo`, `dispose()` returns a `Promise<void>`, and wallet roles resolve from `PathContext.walletDescriptor` (bare x-only hex or `tr(<key>)`).

## 0.0.6

### Patch Changes

- 0f7f94f: Target backend 0.3.10 in the x-satora-server-version header and align the
  Arkade SDK dependency on 0.4.66.

## 0.0.5

### Patch Changes

- 9f4d595: Add package READMEs for `@satora/escrow` and `@satora/swap`.
- bbba274: Add package README (published to npm).

## 0.0.5-rc.0

### Patch Changes

- 9f4d595: Add package READMEs for `@satora/escrow` and `@satora/swap`.
- bbba274: Add package README (published to npm).
