# @satora/escrow-client

High-level escrow flows for [Arkade](https://arkade.sh): fund a 2-of-2 escrow
from Lightning, and withdraw a released payout to Lightning or to L1. It bundles
[`@satora/escrow`](../escrow) (the escrow primitives, all re-exported) with the
escrow monitor, and an **injected** swap client.

You set up two things — a Satora swap client and an `EscrowClient` — and this
README walks through both from scratch. Everything else (escrow primitives, the
monitor) comes from this package.

## Install

```sh
npm install @satora/escrow-client
```

Peer dependencies (you provide these):

```sh
npm install @arkade-os/sdk @satora/swap
```

- `@arkade-os/sdk` — Ark providers, repositories, and the `Wallet` used for withdrawals.
- `@satora/swap` — the Satora swap client: the Lightning/L1 on/off-ramp. You
  build one (shown below) and inject it; escrow-client only imports its type, so it
  carries none of that bundle's runtime weight.

## Setup

Two pieces: a **swap client** and an **`EscrowClient`**.

### 1. Build the swap client

```ts
import { Client } from "@satora/swap";

// The minimal form generates a fresh ephemeral wallet on build(). For anything
// real, persist it — pass .withMnemonic("abandon abandon …") (or a storage
// backend) so pending swaps survive restarts.
const swap = await Client.builder()
  .withBaseUrl("https://api.satora.io") // the Satora API
  .build();
```

### 2. Build the EscrowClient

It needs the swap client plus Ark providers and repositories. Create one for the
lifetime of your app.

```ts
import {
  RestArkProvider,
  RestIndexerProvider,
  InMemoryContractRepository,
  InMemoryWalletRepository,
  networks,
} from "@arkade-os/sdk";
import { EscrowClient } from "@satora/escrow-client";

const ARK_URL = "https://master.arkade.sh"; // the ASP (serves Ark + indexer)

const arkProvider = new RestArkProvider(ARK_URL);
const indexerProvider = new RestIndexerProvider(ARK_URL);

const escrowClient = await EscrowClient.create({
  swap,
  arkProvider,
  indexerProvider,
  contractRepository: new InMemoryContractRepository(),
  walletRepository: new InMemoryWalletRepository(),
});

// Resolve the network from the ASP (used for address derivation below).
const info = await arkProvider.getInfo();
const network = networks[info.network as keyof typeof networks];
```

> Works the same in the browser — build the providers and the swap client there
> (the SDK ships browser storage backends) and pass them in.

## Fund an escrow from Lightning

Describe the escrow you want to fund as `EscrowScriptOptions` (x-only 32-byte
pubkeys; `exitTimelock` is the ASP-mandated CSV). `fundFromLightning` derives the
escrow address, starts watching it, and creates a Lightning→Arkade swap whose
payout claims into that escrow.

```ts
import type { EscrowScriptOptions } from "@satora/escrow-client"; // re-exported from @satora/escrow

const escrow: EscrowScriptOptions = {
  sellerPubKey,            // Uint8Array(32)
  arbiterPubKey,           // Uint8Array(32)
  aspPubKey,               // Uint8Array(32) — the ASP's signer pubkey
  exitTimelock: {type: "blocks", value: 144n},
};

const handle = await escrowClient.fundFromLightning({
  escrow,
  network,
  amountSats: 50_000, // sats to receive at the escrow
});

console.log("pay this invoice:", handle.invoice);
console.log("escrow address:", handle.escrowAddress);

// After the invoice is paid: claim the swap into the escrow and wait for the
// VTXO to land (rejects on timeout).
const funded = await handle.awaitFunded(120_000);
console.log("escrow funded:", funded.contract.script);
```

`handle` is `{ swapId, invoice, escrowAddress, awaitFunded(timeoutMs?) }`.

## Withdraw a released payout

Once the escrow has been cooperatively released to the recipient's wallet, the
payout sits as a normal VTXO in their `@arkade-os/sdk` `Wallet`. You hand that
`Wallet` to the withdrawal methods — build it from the recipient's key and the
same providers:

```ts
import { Wallet, SingleKey } from "@arkade-os/sdk";

const wallet = await Wallet.create({
  identity: SingleKey.fromPrivateKey(recipientSecretKey), // Uint8Array(32)
  arkProvider,
  indexerProvider,
  // ...onchain provider + repositories as your environment needs
});
```

Then withdraw it — either with the smart `withdraw` (which auto-routes by
inspecting the destination) or with a specific method.

### Smart withdraw (auto-route)

`withdraw` figures out where `destination` points and dispatches accordingly:

```ts
const result = await escrowClient.withdraw({
  wallet,
  destination,        // BOLT11 / LNURL / user@host → Lightning
                      // ark1… / tark1…             → Arkade transfer
                      // bc1… / tb1… / 1…           → L1 offboard
  amountSats,         // required for Arkade + LNURL/address; optional for L1; ignored for BOLT11
});

result.txid;          // present on every branch
if (result.method === "lightning") result.swapId; // lightning also has swapId + sourceAmountSats
```

`result` is discriminated by `method` (`"lightning" | "l1" | "arkade"`). Use the
specific methods below if you already know the destination type.

### To Lightning

`destination` may be a **BOLT11 invoice**, an **LNURL** (`lnurl1…`), or a
**Lightning address** (`user@host`). For LNURL / address the swap backend
resolves and negotiates the invoice, so pass `amountSats` (what the recipient
receives); for a BOLT11 invoice the amount is in the invoice and `amountSats` is
ignored.

```ts
// Optional: quote the recipient amount for a full-payout withdrawal
// (payout minus the swap fee) so you don't have to do fee math.
// `availableSats` is the spendable payout in your wallet.
const {recipientSats} = await escrowClient.quoteLightningWithdrawal(availableSats);

const {swapId, fundingTxid, sourceAmountSats} =
  await escrowClient.withdrawToLightning({
    wallet,                                  // @arkade-os/sdk Wallet holding the payout
    destination: "user@lnurl.example.com",   // invoice | lnurl1… | user@host
    amountSats: recipientSats,               // required for LNURL / address
  });
```

`sourceAmountSats` is what was spent from the payout (recipient amount + swap fee);
the swap server then pays the recipient.

### To L1 (onchain)

A collaborative Arkade offboard (settlement round) to an onchain address:

```ts
const settlementTxid = await escrowClient.withdrawToL1({
  wallet,
  destinationAddress: "bc1q…",
  // amountSats?: bigint  // omit to offboard the whole payout
});
```

### To another Arkade address

A plain offchain Ark transfer — the funds stay on Ark, so it's the cheapest and
fastest withdrawal (no swap, no settlement round):

```ts
const arkTxid = await escrowClient.withdrawToArkade({
  wallet,
  destinationAddress: "ark1…", // or tark1… off mainnet
  amountSats: 50_000,
});
```

## Escrow primitives

`@satora/escrow-client` re-exports everything from `@satora/escrow`, so you can
build/verify/sign escrow transactions and access the monitor without a second
import:

```ts
import {
  EscrowVtxoScript,        // 2-of-2 escrow VtxoScript
  buildEscrowReleaseTx,    // build the cooperative release ark-tx
  signEscrowArkTx,         // sign a release as a co-party
  verifyReleaseArkTx,      // verify a release pays the agreed outputs
} from "@satora/escrow-client";

// The underlying monitor (onFunded/onReleased, watch, listEscrows):
escrowClient.escrowMonitor;
```

## Cleanup

```ts
escrowClient.dispose(); // stop watching, clear listeners
```

## API summary

| Method                                                         | Purpose                                                                                                  |
| -------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `EscrowClient.create(config)`                                  | Construct with `{ swap, arkProvider, indexerProvider, contractRepository, walletRepository }`.           |
| `fundFromLightning({ escrow, network, amountSats })`           | Create a LN→escrow swap; returns `{ swapId, invoice, escrowAddress, awaitFunded() }`.                    |
| `withdraw({ wallet, destination, amountSats? })`               | Smart withdrawal — auto-routes to Lightning / L1 / Arkade by destination. Returns `{ method, txid, … }`. |
| `quoteLightningWithdrawal(sourceAmountSats)`                   | `{ recipientSats, sourceSats }` — recipient amount after the swap fee.                                   |
| `withdrawToLightning({ wallet, destination, amountSats? })`    | Withdraw the payout to a BOLT11 / LNURL / Lightning address.                                             |
| `withdrawToL1({ wallet, destinationAddress, amountSats? })`    | Withdraw the payout onchain via collaborative offboard.                                                  |
| `withdrawToArkade({ wallet, destinationAddress, amountSats })` | Withdraw the payout to another Arkade address (offchain Ark transfer).                                   |
| `escrowMonitor`                                                | The underlying `EscrowMonitor`.                                                                          |
| `dispose()`                                                    | Release monitor resources.                                                                               |
