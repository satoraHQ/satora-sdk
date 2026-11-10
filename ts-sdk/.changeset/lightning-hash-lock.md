---
"@satora/swap": patch
---

Check that a Lightning invoice pays the swap's hash lock at creation and when
recovery stores a swap for the first time, in every Lightning direction
except EVM-sourced ones, and keep a stored swap's hash
locks, lock addresses, invoice payment hashes and keys from being replaced by
a server refresh or recovery, except on EVM-sourced swaps (see the legacy SDK
changeset). `SwapVerificationError` is exported.
