---
"@lendasat/lendaswap-sdk-pure": patch
---

Export `SDK_COMMIT_HASH` — the git commit the SDK was built from, captured at build time by `scripts/sync-version.mjs`. Lets consumers report the exact SDK source revision (e.g. in a version footer). `version.ts` is now a generated, gitignored artifact regenerated on install/build.
