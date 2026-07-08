# @lendasat/lendaswap-sdk-pure

## 0.3.0-rc.1

### Patch Changes

- 0fb68c7: Export the real SDK version from the package entry point. `SDK_VERSION`, `SDK_NAME`, and `CLIENT_AGENT` are now re-exported from the index (sourced from the generated `version.ts`); the stale hard-coded `VERSION = "0.0.1"` export is removed.

## 0.3.0-rc.0

### Minor Changes

- 9f4d595: Add support for EURe in Arbitrum.
  SDK uses new orchestration flow.
