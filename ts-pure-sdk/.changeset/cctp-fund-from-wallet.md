---
"@lendasat/lendaswap-sdk-pure": minor
---

`fundSwap` now funds a CCTP-inbound swap (USDC from Base, Optimism, Avalanche,
… to Bitcoin) straight from a browser wallet. The wallet sends the approve and
burn on the source chain; for a swap created with `gasless: true`, the
Arbitrum settlement is owned by the SDK's own key, which can sign the
EIP-7702 authorization browser wallets usually cannot. A swap whose depositor
is the wallet itself still settles with the wallet.

The burn is recorded on the stored swap (`cctpBurnTxid`) as soon as it is
broadcast. Calling `fundSwap` again after an interruption resumes at the
attestation instead of burning twice, and the new
`recoverCctpInboundSwap(swapId, recipient)` recovers the USDC from that burn
if the settlement never lands. `cctpFundSwap` gains `ownerSigner`,
`burnTxHash` and `onBurn` for callers composing the flow themselves.
