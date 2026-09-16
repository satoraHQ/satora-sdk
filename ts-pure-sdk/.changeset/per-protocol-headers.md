---
"@lendasat/lendaswap-sdk-pure": minor
---

Send one protocol compatibility epoch header per protocol component (`bitcoin-htlc`, `arkade-vhtlc`, `lightning`, `evm-erc20-htlc`, `evm-native-htlc`) on every request instead of the legacy server-version header. Exports `PROTOCOL_HEADERS`, `PROTOCOL_VERSIONS`, `ProtocolComponent` and `ProtocolEpoch`.
