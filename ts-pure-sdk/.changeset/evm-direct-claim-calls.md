---
"@lendasat/lendaswap-sdk-pure": patch
---

A direct EVM claim (the locked token is paid out as is) refuses a
`calls_hash` other than that of an empty call list, as the Rust SDK does.
