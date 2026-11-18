---
"@lendasat/lendaswap-sdk-pure": patch
---

Apply the client-level default referral code (`withReferralCode` / `withOrgCode`) to every create, not only `createSwap`. The direct `create*Swap` methods (e.g. `createArkadeToLightningSwap`) now send it when no per-call `referralCode` is given, so a create is priced with the same fees as the matching `getQuote`. This fixes fixed-lock BOLT11 swaps for referral keys with discounted or marked-up fees, and keeps fee attribution for integrators that use the direct methods. A direct `composeQuote` call now falls back to the default as well. A per-call `referralCode` still wins.
