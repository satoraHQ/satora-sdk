---
"@satora/swap": minor
---

Add a derived next-action model with observe-mode tracking, so consumers no
longer have to re-infer UX from the raw 16-state `SwapStatus`.

Call `client.startTracking()` and subscribe with `client.subscribeToActions(cb)`
to receive the recommended next action for each of your swaps — `fund`, `wait`,
`claim`, `refund`, or `none` — recomputed as the chain state changes. The state
is derived **purely from on-chain observations** (per-ledger contract managers
watching each HTLC), never from the server's status, so it also works for
recovery when the API is unavailable. Each leg's funding amount, token and
recipient are verified, so the client is never told to claim a leg funded on the
wrong terms.

Covers every swap direction: Arkade↔EVM, Bitcoin↔EVM, `btc_to_arkade`, and all
four Lightning directions. Tracking is on by default (with sensible RPC, esplora,
and Arkade endpoints) and is overridable or disableable via the `Client` builder.
