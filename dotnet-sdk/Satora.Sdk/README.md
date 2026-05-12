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
    esploraUrl: "http://localhost:3000");
```

### Network defaults

| `BitcoinNetwork` | `baseUrl`                            | `arkadeServerUrl`             | `esploraUrl`                |
| ---------------- | ------------------------------------ | ----------------------------- | --------------------------- |
| `Mainnet`        | `https://api.satora.io`              | `https://arkade.computer`     | `https://mempool.space/api` |
| `Testnet`        | `https://mutinynetswap.lendasat.com` | `https://mutinynet.arkade.sh` | `https://mutinynet.com/api` |
| `Signet`         | `https://mutinynetswap.lendasat.com` | `https://mutinynet.arkade.sh` | `https://mutinynet.com/api` |
| `Regtest`        | `http://localhost:3333`              | `http://localhost:7070`       | `http://localhost:3000`     |

`Testnet` and `Signet` both route to mutinynet — vanilla Bitcoin testnet
isn't supported by the Arkade infrastructure.

## Supported swap pairs

The SDK currently exposes **EVM-stablecoin → BTC on Arkade** through
`CreateSwapAsync`. Other directions the backend supports (Lightning ↔
Arkade, Bitcoin ↔ EVM, Arkade → EVM, etc.) will land as we add
direction-specific entry points — open an issue if you need one
prioritised.

Source tokens (use as `sourceToken` with the matching `sourceChain`):

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
supported on every pair.

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
standard `try`/`catch` works. The inner `Kind` enum carries a tagged
variant identifying the underlying Rust error.

## Links

- Source: <https://github.com/satoraHQ/satora-sdk>
- API host: <https://api.satora.io>
- License: MIT
