---
"@lendasat/lendaswap-sdk-pure": minor
---

Refund EVM-sourced swaps without the server.

`refundSwap` accepts an EVM signer and builds the refund from the funding
transaction's `SwapCreated` log instead of asking the server for calldata, on
the EVM → Arkade, Bitcoin and Lightning directions. The log's sender decides
the shape, the coordinator's `refundTo` or a direct `HTLCErc20.refund` for an
HTLC the wallet created itself, and the HTLC is checked to be active on-chain
first. The funding tx is taken from the client's own record, then the server's
copy; the server is asked for another txid only when none of those yields a
receipt, and for calldata once the chain path has failed. When both fail the
result is `success: false` with both reasons, and a transport error on the
calldata request is reported the same way instead of thrown. `refundSwap`
throws when the signer is on another chain than the swap.

`fundSwap` (on the direct Permit2 path; CCTP-inbound funding is not covered)
and `fundSwapGasless` record the funding transaction and the coordinator on the
stored swap as `StoredSwap.evmFundTxid` and `evmCoordinatorAddress`, which
survive server refreshes and swap recovery; the SQLite storage gains two
columns for them. `fundSwap` returns the mined hash when the wallet replaced
the transaction and rejects a replacement that did not create the HTLC.
`TxReceipt` gains an optional `logs` field that signer implementations must
fill for this path (`fundSwap` logs a warning when it is missing), and
`getTransaction` must reject for a hash the node does not know.

Before building through the coordinator, the client reads its `deposits(key)`
and requires the depositor to be the wallet, one of the SDK's gasless keys or
the depositor the server recorded; anything else fails the chain path.
`evmRefundData.recipient` names where the refund pays out, and the message
points at `recoverGaslessFunds` when that is an SDK key. `fundSwap` records a
replacement only if its receipt carries this swap's `SwapCreated` from the
coordinator on the HTLC.
