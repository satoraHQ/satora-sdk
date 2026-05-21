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
