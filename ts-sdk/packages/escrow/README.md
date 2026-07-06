# @satora/escrow

Low-level **2-of-2 escrow primitives** for [Arkade](https://arkade.sh): derive the
escrow `VtxoScript`, build/sign/verify the cooperative release transaction, and
watch escrows on-chain.

> **Most apps want [`@satora/escrow-client`](https://www.npmjs.com/package/@satora/escrow-client)**,
> the high-level flows (fund an escrow from Lightning, withdraw a released payout
> to Lightning / L1 / Arkade). It re-exports everything here, so you only need
> `@satora/escrow` directly when you're building custom escrow logic.

## Install

```sh
npm install @satora/escrow
```

Peer dependency:

```sh
npm install @arkade-os/sdk
```

## Describe an escrow

An escrow is a 2-of-2 between a seller and an arbiter, spendable via the Arkade
server. `EscrowScriptOptions` describes it (x-only 32-byte pubkeys; `exitTimelock`
is the server-mandated CSV):

```ts
import { EscrowVtxoScript, type EscrowScriptOptions } from "@satora/escrow";

const options: EscrowScriptOptions = {
  sellerPubKey,          // Uint8Array(32)
  arbiterPubKey,         // Uint8Array(32)
  arkadeServerPubKey,    // Uint8Array(32) — the Arkade server's signer pubkey
  exitTimelock: { type: "blocks", value: 144n },
};

const script = new EscrowVtxoScript(options);
// → the escrow address, scripts, and forfeit/exit paths
```

## Release flow

A cooperative release pays the escrowed VTXO to the agreed outputs. Build it,
have each co-party sign, verify it pays what was agreed, then submit:

```ts
import {
  buildEscrowReleaseTx,   // build the release Arkade transaction
  signEscrowArkTx,        // sign a release as a co-party
  verifyReleaseArkTx,     // check the release pays the agreed outputs
  submitAndFinalizeEscrowRelease,
} from "@satora/escrow";
```

- `buildEscrowReleaseTx` → the unsigned release + its expectations.
- `verifyReleaseArkTx` → reject a release that doesn't pay the agreed outputs
  (throws `ReleaseArkTxValidationError`).
- `signEscrowArkTx` / `signEscrowReleaseInPlace` / `signEscrowCheckpoints` →
  produce the co-party signatures.
- `submitAndFinalizeEscrowRelease` → submit to the Arkade server and finalize.

## Monitor

`EscrowMonitor` watches escrow addresses and emits funded / released events:

```ts
import { EscrowMonitor } from "@satora/escrow";
// onFunded / onReleased callbacks, watch(address), listEscrows()
```

## License

MIT
