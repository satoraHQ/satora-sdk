# @lendasat/lendaswap-sdk-pure

## 0.4.0

### Minor Changes

- e42e8c8: Add an optional `invoiceDescription` to `createLightningToEvmSwapGeneric` and `createLightningToArkadeSwap`.

  Sets the text shown in the payer's wallet when they open the Lightning invoice. When omitted, the server applies a branded default (e.g. `Satora swap to USDC on Optimism`); an explicit empty string blanks the description. Backed by the new optional `invoice_description` field on the `POST /swap/lightning/evm` and `POST /swap/lightning/arkade` endpoints.

### Patch Changes

- 1db87ef: Recommend `@satora/swap` for new code. This is now the legacy SDK — it stays
  fully supported, but `@satora/swap` is a drop-in replacement (same API; just
  change the package name in your imports) and is where new features land. We
  intend to deprecate this package and migrate consumers over to `@satora/swap`.
- eb07502: Regenerate the OpenAPI types (adding the `btc_to_arkade` and Lightning swap
  response fields) and align `@arkade-os/sdk` to `^0.4.45`.
- 57a0d76: Fix gasless EVM collaborative refunds to sign with the actual coordinator depositor key.

## 0.3.0

### Minor Changes

- 43a6fc7: Add `getBulkStatus(ids)` to fetch the status of many swaps in a single request.

  Returns `{ statuses, not_found }` — only each swap's status, so the whole batch is served by one database query. Unknown IDs are returned in `not_found` instead of throwing, so one bad ID does not fail the whole call. Backed by the new `POST /swap/bulk-status` endpoint (max 100 IDs per request).

- 9f4d595: Add support for EURe in Arbitrum.
  SDK uses new orchestration flow.

### Patch Changes

- 0fb68c7: Export the real SDK version from the package entry point. `SDK_VERSION`, `SDK_NAME`, and `CLIENT_AGENT` are now re-exported from the index (sourced from the generated `version.ts`); the stale hard-coded `VERSION = "0.0.1"` export is removed.
- ed3d6d8: Export `SDK_COMMIT_HASH` — the git commit the SDK was built from. It's injected into the generated `version.ts` at build time from the `GIT_COMMIT_HASH` env var (set in CI on publish, same convention as the backend), and defaults to `"unknown"` for local builds. Lets consumers report the exact SDK source revision (e.g. in a version footer).

## 0.3.0-rc.2

### Patch Changes

- ed3d6d8: Export `SDK_COMMIT_HASH` — the git commit the SDK was built from. It's injected into the generated `version.ts` at build time from the `GIT_COMMIT_HASH` env var (set in CI on publish, same convention as the backend), and defaults to `"unknown"` for local builds. Lets consumers report the exact SDK source revision (e.g. in a version footer).

## 0.3.0-rc.1

### Patch Changes

- 0fb68c7: Export the real SDK version from the package entry point. `SDK_VERSION`, `SDK_NAME`, and `CLIENT_AGENT` are now re-exported from the index (sourced from the generated `version.ts`); the stale hard-coded `VERSION = "0.0.1"` export is removed.

## 0.3.0-rc.0

### Minor Changes

- 9f4d595: Add support for EURe in Arbitrum.
  SDK uses new orchestration flow.
