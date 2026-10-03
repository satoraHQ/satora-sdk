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
entry point, `CreateArkadeToLightningSwapAsync`. Other directions the
backend supports (Bitcoin ↔ EVM, Arkade → EVM, etc.) will land as we add
direction-specific entry points; open an issue if you need one
prioritised.

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

`Kind` is not a retry signal. After a `Transport` or `Timeout` error from
`SendArkadeAsync`, `FundSwapAsync` or `SettleArkadeAsync`, or any
`Storage` error from `FundSwapAsync`, the outcome is unknown: check the
swap status or the balance before trying again (a `FundSwapAsync` retry
is refused while its stored operation is unresolved, so checking first is
safe). After one from `ClaimAsync`, or a `Storage` error from it once
arkd may hold the claim, call it again, which resumes from its journal.
After one from `RefundBitcoinAsync`, or a `Storage` error from it after a
fee bump, call it again with the same destination, which returns the
pending or confirmed refund instead of building another. A chain read
that straddled a new block, such as
`Bitcoin tip changed during observation`, is a `Transport` error too, so
the same retry applies.

The Arkade wallet's own spends (`SendArkadeAsync` and `SettleArkadeAsync`)
take one lock per `Client`, which claims and refunds never wait for. Each
holds it for at most 120 seconds (`SendArkadeAsync`) or 300 seconds
(`SettleArkadeAsync`) and then fails with `Timeout`.

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
