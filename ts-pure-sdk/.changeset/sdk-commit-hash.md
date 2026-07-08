---
"@lendasat/lendaswap-sdk-pure": patch
---

Export `SDK_COMMIT_HASH` — the git commit the SDK was built from. It's injected into the generated `version.ts` at build time from the `GIT_COMMIT_HASH` env var (set in CI on publish, same convention as the backend), and defaults to `"unknown"` for local builds. Lets consumers report the exact SDK source revision (e.g. in a version footer).
