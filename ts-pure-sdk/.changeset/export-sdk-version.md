---
"@lendasat/lendaswap-sdk-pure": patch
---

Export the real SDK version from the package entry point. `SDK_VERSION`, `SDK_NAME`, and `CLIENT_AGENT` are now re-exported from the index (sourced from the generated `version.ts`); the stale hard-coded `VERSION = "0.0.1"` export is removed.
