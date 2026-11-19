# Lendaswap Client SDK

Monorepo containing client SDKs for Lendaswap - Bitcoin-to-stablecoin atomic swaps.

## Structure

This repository contains interconnected packages:

### [`rust-sdk/`](./rust-sdk/) - Satora Rust SDK

Use `satora-rust-sdk` for Rust integrations. The Rust library name is `lendaswap_sdk`. It handles swap creation, key management, and recovery across Bitcoin, Arkade, Lightning, and EVM chains. See the [Rust SDK README](./rust-sdk/README.md) for supported routes and examples.

Native Swift and Kotlin apps can use the Rust SDK through bindings you provide.

### [`ts-sdk/`](./ts-sdk/) - Satora TypeScript SDKs

The `@satora/*` packages. The one to build against is
[`@satora/swap`](./ts-sdk/packages/swap/) — the swap client:

- Pure idiomatic TypeScript, works in every JS environment
- HD key management for swap parameters
- Storage providers (LocalStorage, IndexedDB, Memory, SQLite via node-sdk)
- Published as `@satora/swap` on npm
- A drop-in replacement for `@lendasat/lendaswap-sdk-pure` (same API), plus a
  home for Satora-native features

### [`ts-pure-sdk/`](./ts-pure-sdk/) - Internal TypeScript implementation

Private workspace package bundled into `@satora/swap`, including its types and
Node, delegate, and CCTP entry points. It is no longer published separately.
Existing published `@lendasat/lendaswap-sdk-pure` versions remain available;
use `@satora/swap` for new releases.

Add changesets in `ts-sdk/.changeset`, targeting `@satora/swap`, for changes to
either source directory. The SDK identity and version come from `@satora/swap`.

## Compatibility headers

SDK HTTP clients send `X-Lendaswap-Client: <sdk-name>/<semver>` plus one integer protocol epoch per protocol component on every request: `x-satora-bitcoin-htlc-version`, `x-satora-arkade-vhtlc-version`, `x-satora-lightning-version`, `x-satora-evm-erc20-htlc-version` and `x-satora-evm-native-htlc-version`. The server checks the components a request's swap uses and rejects clients whose epoch for one of them is too old.

## Building

This project uses [Just](https://github.com/casey/just) as a command runner.

```bash
# Build the TypeScript SDK
just build
```

## License

MIT
