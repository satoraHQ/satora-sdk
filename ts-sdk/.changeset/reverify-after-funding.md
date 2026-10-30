---
"@satora/swap": patch
---

A funded EVM swap advances as soon as `fundSwap` resolves.

`fundSwap` now re-verifies the swap against the chain once the funding
transaction has mined, so the derived action moves off `fund` without waiting
for the server's status push. On Rootstock that push follows a 30 s poll plus
the configured confirmations, during which the deposit step looked stuck.

A swap tracked after start is put on the hint feed when it is tracked, not
only once it emits its first action. On a chain whose public RPC serves no
`eth_getLogs` an unfunded lock cannot be classified, so such a swap emitted
nothing and never received the server's `clientfunded` hint.
