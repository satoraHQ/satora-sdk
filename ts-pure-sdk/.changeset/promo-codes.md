---
"@lendasat/lendaswap-sdk-pure": minor
---

Add promo codes: pass `promoCode` to `getQuote`, `getLightningSendQuote`, `composeQuote`, `getReferralFee` and every `createSwap` direction to replace the base fee for that swap. Quotes priced with a code carry it as `promo` (`code`, `fee_rate`, `max_amount_sats`).
