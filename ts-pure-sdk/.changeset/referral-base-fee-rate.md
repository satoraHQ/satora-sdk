---
"@lendasat/lendaswap-sdk-pure": patch
---

`/referral-fee` may return `base_fee_rate`, a negotiated partner rate that replaces the per-pair `fee_percentage`. `composeQuote` applies it before `extra_fee_rate`, matching server quotes and swap creation for keys with a base fee override.
