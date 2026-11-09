---
"@satora/swap": patch
---

Type the new fields `GET /swap/{id}` returns for EVM-sourced swaps (`evm_to_bitcoin`, `evm_to_lightning` and `evm_to_arkade`): `evm_htlc_locked_amount` (the amount the HTLC holds, as observed from its `SwapCreated` event) and `evm_htlc_sender` (the address the lock is keyed under), both `null` until the lock is seen. `evm_to_arkade` also gains `wbtc_address`, the token locked in the HTLC. With these, a REST client can build a unilateral refund without parsing the creation log.
