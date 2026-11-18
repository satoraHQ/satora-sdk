---
"@satora/swap": patch
---

The default referral code set with `withReferralCode` / `withOrgCode` now also applies to the direct `create*Swap` methods (e.g. `createArkadeToLightningSwap`), so quote and create are priced with the same fees. A per-call `referralCode` still wins.
