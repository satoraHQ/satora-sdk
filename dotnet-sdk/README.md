# Lendaswap.Sdk (.NET)

C# / .NET bindings for the Lendaswap Client SDK. Thin wrapper over a Rust
core (`../rust-sdk`) via [UniFFI](https://github.com/mozilla/uniffi-rs).

## Architecture

```
+----------------------------+
|  Lendaswap.Sdk (C#)        |   ← idiomatic C# facade
|  Client / Version / ...    |
+----------------------------+
              │ (Task.Run wrap)
              ▼
+----------------------------+
|  Generated/                |   ← uniffi-bindgen-cs output
|  lendaswap_sdk_ffi.cs      |     (regenerated from the cdylib)
+----------------------------+
              │ P/Invoke
              ▼
+----------------------------+
|  native/  (Rust cdylib)    |   ← UniFFI exports
|  liblendaswap_sdk_ffi.*    |
+----------------------------+
              │ Rust path dep
              ▼
+----------------------------+
|  ../rust-sdk               |   ← pure Rust SDK
+----------------------------+
```

## Prerequisites

- **Rust** — the repo's pinned toolchain (Rust 1.94, see `rust-toolchain.toml`)
- **.NET SDK 8.0+** — `dotnet` on PATH
- **`uniffi-bindgen-cs`** — installed once via `just install-bindgen`

## Quick start

```bash
just install-bindgen      # one-time, pins uniffi-bindgen-cs v0.10.0+v0.29.4
just build                # builds the cdylib + generates C# + dotnet build
just test                 # runs xUnit tests (live tests skipped unless env set)
just test-live            # hits http://localhost:3333 by default
```

## Adding a method to the FFI surface

1. Write the function in `native/src/lib.rs` with `#[uniffi::export]`.
   Use `runtime().block_on(...)` to drive async `lendaswap-sdk` methods
   from a synchronous extern boundary.
2. `just build-native && just generate-bindings` — regenerates
   `Lendaswap.Sdk/Generated/lendaswap_sdk_ffi.cs`.
3. Add an idiomatic facade in `Lendaswap.Sdk/` that wraps the generated
   binding in a `Task.Run(...)` so callers see a `Task<T>` API.
4. Add an xUnit test under `Lendaswap.Sdk.Tests/`.

## Version pinning

`uniffi` (the Rust crate) and `uniffi-bindgen-cs` (the external code
generator) **must agree on version**. Release tags on
`uniffi-bindgen-cs` are formatted `<bindgen>+<uniffi>`; the `Justfile`
pins to `v0.10.0+v0.29.4`. When bumping `uniffi` in `native/Cargo.toml`,
bump `bindgen_tag` in the Justfile in lockstep.

## NuGet packaging

`just pack` builds a `.nupkg` against the currently-compiled native lib
(host RID only). For real publishing, CI cross-compiles
`liblendaswap_sdk_ffi.*` for every supported RID (`osx-arm64`,
`osx-x64`, `linux-x64`, `linux-arm64`, `win-x64`) and drops them under
`native/target/<rid>/release/` before `pack` runs. The `.csproj` packs
them under `runtimes/<rid>/native/` so the .NET host's RID-specific
asset resolution finds them at load time.

## Live integration tests

`Lendaswap.Sdk.Tests/ClientTests.LiveVersionReturnsNonEmptyFields` hits
`LENDASWAP_API_URL` (defaults to skipped). Run `just test-live` to point
at a local server, or pass the env var explicitly:

```bash
LENDASWAP_API_URL=https://api.satora.io dotnet test
```
