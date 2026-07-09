---
"@satora/swap": minor
---

`@satora/swap` is now a standalone, drop-in swap client instead of a bare
re-export of `@lendasat/lendaswap-sdk-pure`. `Client` and `ClientBuilder` wrap
the underlying legacy client with the exact same public API, and this is where
new Satora-native features will land.

**This is the recommended swap package going forward.** We intend to deprecate
`@lendasat/lendaswap-sdk-pure` and migrate all consumers over to `@satora/swap`.
Migrating is a drop-in change — swap the package name in your imports, nothing
else changes. The legacy package stays supported throughout the transition.
