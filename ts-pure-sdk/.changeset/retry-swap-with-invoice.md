---
"@lendasat/lendaswap-sdk-pure": minor
---

Retry a stuck EVM → Lightning swap with a new invoice instead of refunding. `getRehashEvmParams` reports the lock and what the new invoice must satisfy; `retrySwapWithInvoiceWithSigner(swapId, invoice, signer)` has the wallet authorize the move (`personal_sign`), fetches the server's `Rehash` signature and sends `rehash` on the HTLC; `retrySwapWithInvoice(swapId, invoice)` does the same from the SDK key as a sponsored UserOp on Arbitrum. Also exports `invoicePaymentHash`, `rehashAuthorizationMessage` and `signEvmMessage` (EIP-191).
