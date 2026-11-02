---
"@satora/swap": patch
---

A funded EVM swap advances as soon as `fundSwap` resolves.

With chain-verified tracking, `fundSwap` now re-verifies the swap against the
chain once the funding transaction has mined, so the derived action moves off
`fund` without waiting for the server's status push. On Rootstock that push
follows a 30 s poll plus the configured confirmations, during which the
deposit step looked stuck. Hints-only tracking is unchanged: it has no chain
to read.

A swap tracked after start is put on the hint feed when it is tracked, not
only once it emits its first action; a swap the tracker already saw finish is
not re-subscribed. On a chain whose public RPC serves no
`eth_getLogs` an unfunded lock cannot be classified, so such a swap emitted
nothing and never received the server's `clientfunded` hint.
