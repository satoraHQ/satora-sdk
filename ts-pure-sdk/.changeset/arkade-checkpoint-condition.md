---
"@lendasat/lendaswap-sdk-pure": patch
---

Fix Arkade VHTLC claims rejected with `INVALID_PSBT_INPUT (5): checkpoint condition witness does not match ark tx input 0`: the preimage is now set on every checkpoint before submit, as current arkd requires. Bump `@arkade-os/sdk` to `^0.5.1`.
