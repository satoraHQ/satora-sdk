# Lendaswap Client SDK

Monorepo containing client SDKs for Lendaswap - Bitcoin-to-stablecoin atomic swaps.

## Structure

This repository contains interconnected packages:

### [`core/`](./core/) - Rust Core Library

Platform-agnostic Rust library containing:

- API client for the Lendaswap backend
- Type definitions matching the backend API schema
- HTTP request handling with `reqwest`

### [`ts-sdk/`](./ts-sdk/) - Satora TypeScript SDKs (recommended)

The `@satora/*` packages. The one to build against is
[`@satora/swap`](./ts-sdk/packages/swap/) — the swap client:

- Pure idiomatic TypeScript, works in every JS environment
- HD key management for swap parameters
- Storage providers (LocalStorage, IndexedDB, Memory, SQLite via node-sdk)
- Published as `@satora/swap` on npm
- A drop-in replacement for `@lendasat/lendaswap-sdk-pure` (same API), plus a
  home for Satora-native features

### [`ts-pure-sdk/`](./ts-pure-sdk/) - Legacy TypeScript SDK

Published as `@lendasat/lendaswap-sdk-pure`. **Still fully supported and
working** — `@satora/swap` wraps it — but new code should prefer `@satora/swap`,
where new features land. Existing integrations do not need to migrate urgently.

## Building

This project uses [Just](https://github.com/casey/just) as a command runner.

```bash
# Build the TypeScript SDK
just build
```

## License

MIT
