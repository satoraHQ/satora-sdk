---
"@lendasat/lendaswap-sdk-pure": patch
---

`fundSwap` on a native lock (RBTC on Rootstock) can be retried after the first funding tx reverted, or after the wallet replaced it with a tx whose receipt logs show no lock under the swap's hash. The wallet that sends a native lock is no longer recorded as `StoredSwap.evmCoordinatorAddress`, and on a swap whose creation terms name a direct native lock, a record an older SDK left there is ignored by the coordinator check. A retry is still refused while a recorded funding tx is unknown to the node, not yet mined, mined into the lock, or replaced by a tx whose receipt has no logs or shows a lock under the hash.
