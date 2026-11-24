---
"@satora/swap": patch
---

Count a Bitcoin HTLC funding split over two transactions, as the server does: the Bitcoin readers sum the two largest outputs paying the HTLC address instead of only the largest transaction, so a client that tops up a short payment is tracked as funded, not invalid, and claims once the server has funded.
