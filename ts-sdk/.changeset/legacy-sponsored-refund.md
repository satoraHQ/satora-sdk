---
"@satora/swap": patch
---

`refundEvmSponsored` also refunds gasless swaps created by older SDK versions,
whose depositor is the swap's own key rather than this client's key.
