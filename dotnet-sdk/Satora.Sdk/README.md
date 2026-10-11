# Satora.Sdk

.NET bindings for the [Satora](https://github.com/satoraHQ/satora-sdk)
swap SDK. A thin idiomatic-C# wrapper over a Rust core via
[UniFFI](https://github.com/mozilla/uniffi-rs); native libraries are shipped
per-RID inside the package.

## Install

```bash
dotnet add package Satora.Sdk --prerelease
```

```csharp
using Satora.Sdk;
```

## Quick start

```csharp
using Satora.Sdk;

// One constructor. Mnemonic is required and drives both the EVM
// signer and the Arkade identity (consumers don't get to mismatch
// them). All other knobs default off the chosen network.
using var client = new Client(
    mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");

var version = await client.GetVersionAsync();
Console.WriteLine($"Server: {version.Tag} ({version.CommitHash})");
```

```csharp
// Dev / mutinynet
using var client = new Client(mnemonic, BitcoinNetwork.Signet);

// Full override (rare — e.g. pointing at a local stack)
using var client = new Client(
    mnemonic,
    BitcoinNetwork.Regtest,
    baseUrl: "http://localhost:3333",
    arkadeServerUrl: "http://localhost:7070",
    esploraUrl: "http://localhost:3000/api");
```

### Network defaults

| `BitcoinNetwork` | `baseUrl`                            | `arkadeServerUrl`             | `esploraUrl`                |
| ---------------- | ------------------------------------ | ----------------------------- | --------------------------- |
| `Mainnet`        | `https://api.satora.io`              | `https://arkade.computer`     | `https://mempool.space/api` |
| `Testnet`        | none                                 | none                          | none                        |
| `Signet`         | `https://mutinynetswap.lendasat.com` | `https://mutinynet.arkade.sh` | `https://mutinynet.com/api` |
| `Regtest`        | `http://localhost:3333`              | `http://localhost:7070`       | `http://localhost:3000/api` |

`esploraUrl` is the esplora API root, which Mempool serves under `/api`;
its root serves the web app, so a URL without `/api` fails on the first
chain read.

No Arkade infrastructure runs on testnet, so `Testnet` throws an
`ArgumentException` unless all three URLs are passed. Mutinynet is a
signet: use `Signet` for it.

### Persistent storage

Without `dataDirectory`, swaps live in memory and a client built after a
restart cannot fund or claim them. Pass a directory per wallet and
network to keep them (neither file in it is partitioned by network):

```csharp
using var client = new Client(mnemonic, dataDirectory: "/var/lib/myapp/satora");
```

The directory is created if missing. Swaps go in `swaps.sqlite`, and the
Arkade contract store in `contracts-<wallet id>.sqlite`, with the id
derived from the seed so a different seed restored into the same
directory never shares it. An explicit `contractDbPath` overrides the
latter. Naming the same file as `swaps.sqlite` there throws
`SdkException` with `Kind` `Storage`; the paths are compared after
resolving symlinks, `.` and `..`, and on macOS and Windows letter case. The
mnemonic itself is never written.

Each kind of execution through one `Client`, such as wallet spends or swap
claims, runs one at a time, and different kinds can overlap. Separate
instances are not coordinated, even over the same directory: keep one
`Client` per wallet per process. Calls still running on a disposed
`Client` keep going until they return or time out, so build its
replacement over the same directory only after that.

## Supported swap pairs

`CreateSwapAsync` routes **EVM-stablecoin → BTC on Arkade**, **on-chain
Bitcoin → BTC on Arkade** (see [Bitcoin → Arkade](#bitcoin--arkade)
below) and **Lightning → BTC on Arkade**; Arkade → Lightning has its own
entry point (see [Lightning ↔ Arkade](#lightning--arkade)). Other
directions the backend supports (Bitcoin ↔ EVM, Arkade → EVM, etc.)
will land as we add direction-specific entry points; open an issue if
you need one prioritised.

EVM source tokens (use as `sourceToken` with the matching
`sourceChain`):

| Token | Chain    | TokenId                 | ChainId            |
| ----- | -------- | ----------------------- | ------------------ |
| USDC  | Polygon  | `TokenId.UsdcPolygon`   | `ChainId.Polygon`  |
| USDC  | Arbitrum | `TokenId.UsdcArbitrum`  | `ChainId.Arbitrum` |
| USDC  | Ethereum | `TokenId.UsdcEthereum`  | `ChainId.Ethereum` |
| USDT  | Polygon  | `TokenId.UsdtPolygon`   | `ChainId.Polygon`  |
| USDT  | Ethereum | `TokenId.UsdtEthereum`  | `ChainId.Ethereum` |
| USDT0 | Arbitrum | `TokenId.Usdt0Arbitrum` | `ChainId.Arbitrum` |
| WBTC  | Polygon  | `TokenId.WbtcPolygon`   | `ChainId.Polygon`  |
| WBTC  | Arbitrum | `TokenId.WbtcArbitrum`  | `ChainId.Arbitrum` |
| WBTC  | Ethereum | `TokenId.WbtcEthereum`  | `ChainId.Ethereum` |

Target is always `TokenId.Btc` on `ChainId.Arkade`. `gasless: true` is
supported on every EVM source above; Bitcoin and Lightning sources ignore
it.

Notes:

- Pass `receiveTo: null` to `CreateSwapAsync` to land funds in the
  SDK's own Arkade wallet (the identity derived from your mnemonic).
  Otherwise pass an `Address.Arkade("tark1q…")`.

## Funding swaps (gasless)

For an EVM-funded swap the typical sequence is:

1. `CreateSwapAsync` — backend returns a depositor EOA address.
2. Tell the customer to send the source token there.
3. **Poll `CheckDepositAsync(swapId)` until `HasSufficientSourceToken`**
   — single-shot, returns immediately. No timeout, no throw.
4. `FundSwapAsync(swapId)` — submits the gasless funding userOp.

```csharp
var status = await client.CheckDepositAsync(swapId);
if (status.HasSufficientSourceToken)
{
    var receipt = await client.FundSwapAsync(swapId);
}
else
{
    // show "waiting for customer payment" — try again later
}
```

`FundSwapAsync(swapId)` and `CheckDepositAsync(swapId)` both pick
the node RPC URL from the swap's deposit chain via the same
per-chain defaults.

Defaults come from the Rust core (`KnownChain::default_node_rpc_url`),
which currently points at the public RPCs for Arbitrum / Ethereum /
Polygon — fine for low-volume use. For production volume, custom
paymaster context, or gas overrides, use the explicit overload:

```csharp
var status  = await client.CheckDepositAsync(swapId, nodeRpcUrl: "https://your-private-rpc.example/v3/<key>");
var receipt = await client.FundSwapAsync(
    swapId,
    new GaslessOpts(
        nodeRpcUrl: "https://your-private-rpc.example/v3/<key>",
        paymasterContextJson: null,
        gasOverrides: null));
```

## Bitcoin → Arkade

`CreateBitcoinToArkadeSwapAsync(satsReceive)` creates a swap that pays
exactly `satsReceive` sats on Arkade, to the SDK's own Arkade wallet
unless `receiveTo` is given. `CreateSwapAsync` with `ChainId.Bitcoin`,
`TokenId.Btc` and a `QuoteAmount.Target` does the same.

1. The payer sends `DepositAmount` sats to the on-chain HTLC address in
   `SwapFunding.BitcoinAddress`.
2. Once the server has funded the Arkade VHTLC
   (`SwapStatus.ServerFunded`), `ClaimAsync` sweeps it.
3. If the swap does not complete, `RefundBitcoinAsync` sends the deposit
   to a Bitcoin address of yours once the HTLC's refund locktime has
   matured. It reads the chain through the client's `esploraUrl`.

```csharp
var swap = await client.CreateBitcoinToArkadeSwapAsync(satsReceive: 100_000);
if (swap.Funding is SwapFunding.BitcoinAddress htlc)
{
    Console.WriteLine($"send {swap.DepositAmount} sats to {htlc.address}");
}

// Later, if the swap failed and the refund locktime has passed:
var refund = await client.RefundBitcoinAsync(
    swap.Id, "bc1q…", feeRateSatVb: null, maximumFeeSats: 5_000);
```

## Lightning ↔ Arkade

`CreateLightningToArkadeSwapAsync(satsReceive)` returns a BOLT11 invoice
in `SwapFunding.Bolt11Invoice`. Once it is paid and the server has
funded the Arkade VHTLC, `ClaimAsync` sweeps it. The overload taking a
`QuoteAmount` fixes either the invoice amount (`QuoteAmount.Source`) or
what arrives on Arkade (`QuoteAmount.Target`), and also takes a fee
surcharge and an invoice description. The invoice must be paid before
both `InvoiceExpiresAt` and the claim cutoff `VhtlcRefundLocktime`; the
server does not guarantee which comes first, and `InvoiceExpiresAt` is
null when the server reports no expiry. `ClaimAsync` refuses within 300
seconds of `VhtlcRefundLocktime`. An invoice that does not pay the hash
of the client's preimage throws `InvalidSwap` at creation, and nothing is
stored.

`CreateArkadeToLightningSwapAsync(destination)` returns an Arkade VHTLC
address in `SwapFunding.ArkadeAddress`; `FundArkadeSwapAsync(swap.Id)`
pays it `DepositAmount` sats from the SDK's Arkade wallet and the server
pays the destination. `LightningDestination.AddressSendMax` and
`LightningDestination.LnurlSendMax` take a fixed amount from Arkade and
pay out what remains after fees. `GetLightningSendQuoteAsync` prices a
destination with the Lightning provider's actual fee. If the swap fails,
`SettleAsync` with `ArkadeSpend.CooperativeRefund` (or
`ArkadeSpend.Refund` once median time past, which trails wall clock by
about an hour, has reached the VHTLC refund locktime) returns the funds.
A supplied invoice that does not parse throws `InvalidSwap` before any
request is sent. A hash lock that is not the payment hash of the
returned invoice, or a returned invoice other than the one supplied,
throws `InvalidSwap` at creation, and nothing is stored.

`GetSwapAsync` and `WaitForSwapStatusAsync` throw `InvalidSwap` for a
server copy that names another swap than the one asked for, that changes
the direction of a swap the client stored, or that no longer pays the
hash lock of a Lightning swap the client stored: never pay or fund the
swap then. A swap stored only as its key index, as in an older database,
is checked too: a Lightning → Arkade invoice must pay the hash of the
client's preimage, and an Arkade → Lightning hash lock must be its
invoice's payment hash.

```csharp
var destination = new LightningDestination.Address("alice@example.com", 50_000);
var quote = await client.GetLightningSendQuoteAsync(destination);
var swap = await client.CreateArkadeToLightningSwapAsync(destination);
var arkTxid = await client.FundArkadeSwapAsync(swap.Id);
```

`FundArkadeSwapAsync` journals the payment in the client's storage, so
after a `Transport` or `Timeout` error call it again rather than paying
the VHTLC another way: it returns the payment once it is visible, and
pays again only when the journal proves none went out. It refuses with
`InvalidSwap` a VHTLC locked to a server key other than arkd's current
signer (create a new swap then), a VHTLC amount below arkd's dust, and a
VHTLC whose hash lock is not the payment hash of the swap's invoice.
When an earlier payment may be the swap's and the SDK cannot tell, such
as for a journaled input arkd does not know or a journal for another
VHTLC address, it throws `UnresolvedFunding`: follow the swap's status,
never pay the VHTLC by other means, and give the swap up only once a
check by hand shows where the funds went. `SendArkadeAsync` can spend
inputs a failed funding journaled; once arkd names and returns the send,
which pays nothing to the VHTLC, the next `FundArkadeSwapAsync` resolves
that journal.

## Arkade VHTLCs

`SettleAsync(swapId, operation, destination)` claims
(`ArkadeSpend.Claim`) or refunds (`ArkadeSpend.Refund`,
`ArkadeSpend.CooperativeRefund`) a swap's Arkade VHTLC. Unlike
`ClaimAsync`, it also spends recoverable (expired) outputs, through a
batch, and its `ArkadeSettlement` says which path it took.
`AmountsForSwapAsync(swapId)` shows the spendable, recoverable and spent
sats at the VHTLC. `IsArkadeClaimPossibleAsync(swapId)` says whether a
claim can still complete, without sending the preimage anywhere: `false`
is final, and `true` means `SettleAsync` finishes or resumes the claim.

## Offboarding

`OffboardArkadeAsync(address, amountSats)` sends sats from the SDK's
Arkade wallet to an on-chain Bitcoin address in a collaborative exit.
The server's fee comes out of change, so the address receives exactly
`amountSats`. Every exit keeps at least the server's dust (330 sats
today) in the wallet as Arkade change: ark-client 0.11 registers a
signing key for every exit but signs the batch's VTXO tree only for one
that creates a VTXO, so arkd would fail any batch an exit without change
shares with others. `null` therefore sends everything except the
recoverable balance and that change, the most that leaves at least dust
(less than dust plus one fee step stays behind when the server's fee
program skips the amount that leaves exactly dust), and a fixed amount
that cannot leave dust is refused. `GetArkadeOffboardQuoteAsync` returns
the amount and fee without sending. A server whose fee program charges
the inputs or the change is refused with `InvalidSwap` before anything
is sent, as the collaborative exit pays only the on-chain output's fee.

The exit spends the `confirmedSats` and `preConfirmedSats` of
`GetArkadeBalanceAsync`, as `SendArkadeAsync` and `FundArkadeSwapAsync`
do, taking the soonest-expiring VTXOs first. Recoverable VTXOs and
boarding outputs move only through `SettleArkadeAsync`, so settle before
offboarding them: a batch that mixes an expired, unswept VTXO with
healthy ones fails for every participant. For the same reason VTXOs that
could expire before ark-client's four batch join attempts end (each can
wait out its intent's 120 s validity plus an arkd session; at most the
call's 600 s) stay out until they expire and turn recoverable. On a
server whose VTXOs live minutes, such as regtest's 512 s, that margin is
half the VTXO lifetime, so a retried exit there can still meet an
expiring input. The lifetime is read off the wallet's confirmed VTXOs, so
a wallet that has only received offchain gets the full margin until a
settle or boarding gives it one. When the spendable balance falls short,
the `InvalidSwap` message names the sats left out as expiring or
recoverable. `SendArkadeAsync` and `FundArkadeSwapAsync` leave out the
same expiring VTXOs and refuse a short balance the same way, as a
payment's outputs expire with its soonest-expiring input: a VHTLC paid
from one could expire before the server claims it. `RenewArkadeAsync`
rolls the VTXOs inside the margin into the next batch as fresh ones,
while `SettleArkadeAsync` takes them only once they have expired, so
renew often enough to catch them: on regtest, whose VTXOs live 512 s,
within the last 256 s of their life.

```csharp
var quote = await client.GetArkadeOffboardQuoteAsync("bc1q…", amountSats: null);
Console.WriteLine($"{quote.AmountSats} sats out, {quote.FeeSats} sats fee");
var txid = await client.OffboardArkadeAsync("bc1q…", amountSats: null);
```

## Swap status and recovery

`SwapDetails` carries the swap's `Direction` and, in unix seconds, the
locktimes and invoice expiry that bound when it can be paid, claimed or
refunded. `GetBulkStatusAsync(ids)` reads the status of any number of
swaps in one call. After restoring a seed on a new machine,
`RecoverAllSwapsAsync()` finds the wallet's swaps on the server and
stores them so they can be claimed or refunded.

## Supported runtimes

The package ships native cdylibs for:

- `osx-arm64` (Apple Silicon)
- `osx-x64` (Intel Mac)
- `linux-x64`
- `linux-arm64`
- `win-x64`

Other RIDs are not yet packaged — open an issue if you need one.

## Plugin-host environments (BTCPay Server, etc.)

The SDK installs a `NativeLibrary.SetDllImportResolver` at module load
time that finds `satora_sdk_ffi` under `runtimes/<rid>/native/`
relative to the SDK assembly. A single portable publish works in
plugin hosts that use a custom `AssemblyLoadContext` and don't honor
the consumer's `deps.json` for native libs — no special configuration
needed:

```sh
dotnet publish -c Release       # all RIDs bundled under runtimes/
```

### Legacy: forcing a flat layout

The `<SatoraSdkFlattenNativeLibs>true</SatoraSdkFlattenNativeLibs>`
property and the `build/Satora.Sdk.targets` flatten step are still
available for consumers who need the native lib directly next to the
SDK assembly. Note that this forces one-arch-per-bundle on Linux
because `linux-x64` and `linux-arm64` share the `libsatora_sdk_ffi.so`
filename. Prefer the RID-scoped layout (no flatten) unless you have a
specific reason.

## Errors

All FFI failures surface as `SdkException` (subclass of `Exception`) so
standard `try`/`catch` works. `Kind` names the underlying Rust error and
`Message` carries its text: for `Api`, the server's message, with the
HTTP status in `StatusCode`. A failure inside the bindings, a Rust panic
included, and a Rust SDK error this version does not name yet have
`Kind` `Internal` and keep the original as `InnerException`.

Apart from `UnresolvedFunding`, `Kind` is not a retry signal. After a
`Transport` or `Timeout` error from `SendArkadeAsync`, `FundSwapAsync`,
`SettleArkadeAsync` or `OffboardArkadeAsync`, or any `Storage` error
from `FundSwapAsync`, the outcome is unknown: check the swap status or
the balance before trying again (a `FundSwapAsync` retry throws
`InvalidSwap` when the operation's hash was recorded and
`UnresolvedFunding` when it was not, which no call through this SDK
resolves, so checking first is safe). After one from `ClaimAsync` or
`SettleAsync`, or a `Storage` error from them once arkd may hold the
spend, call it again, which resumes from its journal. After one from
`FundArkadeSwapAsync`, call it again, which resolves the outcome from
its journal. After one from `RefundBitcoinAsync`, or a `Storage` error
from it after a fee bump, call it again with the same destination, which
returns the pending or confirmed refund instead of building another. A
chain read that straddled a new block, such as
`Bitcoin tip changed during observation`, is a `Transport` error too, so
the same retry applies.

`UnresolvedFunding` means an earlier funding of the swap may have gone
out and the SDK cannot tell: never fund the swap by other means, and do
not treat it as unfunded. Of the `FundArkadeSwapAsync` errors, only the
`InvalidSwap` refusal of a swap that no longer awaits funding proves the
wallet paid nothing, provided the storage kept the swap's journal: a
swap recovered on another device, or after a restart without
`dataDirectory`, has none.

The Arkade wallet's own spends (`SendArkadeAsync`, `FundArkadeSwapAsync`,
`SettleArkadeAsync`, `RenewArkadeAsync` and `OffboardArkadeAsync`) take
one lock per `Client`; `GetArkadeOffboardQuoteAsync` waits for it too,
while claims and refunds never do. Each holds it for at most 120 seconds
(`SendArkadeAsync`, `FundArkadeSwapAsync`), 300 seconds
(`SettleArkadeAsync`, `RenewArkadeAsync`, `GetArkadeOffboardQuoteAsync`)
or 600 seconds (`OffboardArkadeAsync`, whose batch join is retried) and
then fails with `Timeout`.

## Cancellation

Every async method runs its native call on the thread pool. Its
`CancellationToken` only stops a call that has not started; a running
native call is not interrupted and runs until it completes or hits its
own timeout.

## Logging

Set `RUST_LOG` (for example `RUST_LOG=info` or
`RUST_LOG=lendaswap_sdk=debug`) in the environment the process is
launched with to write the Rust core's logs to stderr. Logging is off
when it is unset. `Environment.SetEnvironmentVariable` does not reach
the native library on Linux and macOS, so setting it from C# has no
effect there.

## Links

- Source: <https://github.com/satoraHQ/satora-sdk>
- API host: <https://api.satora.io>
- License: MIT
