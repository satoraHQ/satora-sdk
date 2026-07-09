---
"@lendasat/lendaswap-sdk-pure": minor
---

Add an optional `invoiceDescription` to `createLightningToEvmSwapGeneric` and `createLightningToArkadeSwap`.

Sets the text shown in the payer's wallet when they open the Lightning invoice. When omitted, the server applies a branded default (e.g. `Satora swap to USDC on Optimism`); an explicit empty string blanks the description. Backed by the new optional `invoice_description` field on the `POST /swap/lightning/evm` and `POST /swap/lightning/arkade` endpoints.
