---
"@satora/swap": patch
---

Type the new fields `GET /swap/{id}` returns for EVM-sourced swaps (`evm_to_bitcoin`, `evm_to_lightning` and `evm_to_arkade`): `evm_htlc_locked_amount` (the amount the HTLC holds, as observed from its `SwapCreated` event) and `evm_htlc_sender` (the address the lock is keyed under), both `null` until the lock is seen. `evm_htlc_token` names the token locked in the HTLC on all three; `wbtc_address`, which carried the same value, is deprecated but still sent (and now also on `evm_to_arkade`). The SDK reads the lock token from `evm_htlc_token` (falling back to `wbtc_address` only for swaps stored before the server sent it), so relay funding now also binds `evm_to_arkade` payloads to the lock token. With these, a REST client can build a unilateral refund without parsing the creation log.
