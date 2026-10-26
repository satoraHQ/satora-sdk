---
"@lendasat/lendaswap-sdk-pure": minor
---

Estimate gas for funding and refund transactions instead of sending a fixed 500k limit. `EvmSigner` gains an optional `estimateGas`; when provided, the SDK sends with the estimate plus a 30% margin, otherwise it falls back to a static limit (1M for Permit2 funding). A two-hop DEX route on Arbitrum needed ~504k and ran out of gas under the old cap.
