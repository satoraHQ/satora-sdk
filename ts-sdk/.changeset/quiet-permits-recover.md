---
"@satora/swap": patch
---

Normalize Permit2 funding signatures with recovery byte 0/1 to 27/28 when account code confirms an EOA, while preserving compact and contract-wallet signatures. Add optional `EvmSigner.getCode` support for safe account classification; preserve original signatures when account code is unavailable. Direct calldata encoder consumers can supply `depositorCode` to enable normalization.
