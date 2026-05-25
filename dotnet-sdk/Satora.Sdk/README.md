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
- `linux-x64`
- `win-x64`

Other RIDs are not yet packaged — open an issue if you need one.

## Errors

All FFI failures surface as `SdkException` (subclass of `Exception`) so
standard `try`/`catch` works. The inner `Kind` enum carries a tagged
variant identifying the underlying Rust error.

## Links

- Source: <https://github.com/satoraHQ/lendaswap-sdk>
- API host: <https://api.satora.io>
- License: MIT
