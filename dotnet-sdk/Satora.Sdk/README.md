# Satora.Sdk

.NET bindings for the [Satora](https://github.com/satoraHQ/lendaswap-sdk)
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

// Read-only client — version, quotes, status lookups.
using var client = new Client("https://api.satora.io");

var version = await client.GetVersionAsync();
Console.WriteLine($"Server: {version.Tag} ({version.CommitHash})");
```

For swap creation, funding, and claiming you need a signing client:

```csharp
using var client = new Client(
    baseUrl: "https://api.satora.io",
    mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");

// ... CreateSwapAsync / FundSwapAsync / ClaimAsync / etc.
```

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
  SDK's own Arkade wallet (requires the 3-arg client constructor that
  takes an `ArkadeConfig`). Otherwise pass an `Address.Arkade("tark1q…")`.

## Supported runtimes

The package ships native cdylibs for:

- `osx-arm64` (Apple Silicon)
- `osx-x64` (Intel Mac)
- `linux-x64`
- `linux-arm64`
- `win-x64`

Other RIDs are not yet packaged — open an issue if you need one.

## Plugin-host environments (BTCPay Server, etc.)

If you're consuming this SDK from a plugin loaded by a host that uses
a custom `AssemblyLoadContext` and doesn't honor `runtimes/<rid>/native/`
resolution (e.g. BTCPay Server plugins), add this to your csproj and
publish with an explicit RID:

```xml
<PropertyGroup>
  <SatoraSdkFlattenNativeLibs>true</SatoraSdkFlattenNativeLibs>
</PropertyGroup>
```

```bash
dotnet publish -c Release -r linux-arm64 --no-self-contained
```

This copies `libsatora_sdk_ffi.<so|dylib|dll>` flat into the publish
output so the plugin loader's flat search can find it. Use an explicit
RID because `linux-x64` and `linux-arm64` share `libsatora_sdk_ffi.so`
as a filename and would collide after flattening.

## Errors

All FFI failures surface as `SdkException` (subclass of `Exception`) so
standard `try`/`catch` works. The inner `Kind` enum carries a tagged
variant identifying the underlying Rust error.

## Links

- Source: <https://github.com/satoraHQ/lendaswap-sdk>
- API host: <https://api.satora.io>
- License: MIT
