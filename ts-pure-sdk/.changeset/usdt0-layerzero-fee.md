---
"@lendasat/lendaswap-sdk-pure": patch
---

Outbound USDT0 quotes charge the LayerZero messaging fee in sats: the
`layerzero` bridge rate from `/dex-quote` now carries `native_fee_sats`, which
`getQuote` adds to `network_fee`. The bridged USDT0 is no longer reduced by a
token-denominated bridge fee.
