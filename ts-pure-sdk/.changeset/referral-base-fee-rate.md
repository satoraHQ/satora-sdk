---
"@lendasat/lendaswap-sdk-pure": patch
---

`getQuote` uses the server's `/quote` whenever a referral code applies (per call or from the client config) instead of composing locally. A key may carry a negotiated base fee that only the server prices, so composed quotes could disagree with the swap that was then created.
