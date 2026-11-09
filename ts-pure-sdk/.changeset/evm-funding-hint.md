---
"@lendasat/lendaswap-sdk-pure": patch
---

`fundSwap` tells the server which transaction funded an EVM-source swap (`POST /swap/{id}/evm-funding`) once the receipt is in. The server verifies the receipt from its own node and applies the lock as its monitor would, so a funding the monitor missed is picked up at once. Best effort: an older server without the route only loses the shortcut. While the server's node has not seen the transaction yet, the hint is sent again a few times (bounded) before the monitor is left to find the lock.
