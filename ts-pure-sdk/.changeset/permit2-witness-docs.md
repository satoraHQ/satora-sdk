---
"@lendasat/lendaswap-sdk-pure": patch
---

The generated API types now describe the Permit2 witness that `POST /swap/{id}/fund-gasless` expects, and type the `GET /swap/{id}/swap-and-lock-calldata-permit2` response as `Permit2FundingCalldataResponse` instead of a bare string.
