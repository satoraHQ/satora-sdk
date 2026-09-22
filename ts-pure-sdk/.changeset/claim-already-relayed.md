---
"@lendasat/lendaswap-sdk-pure": patch
---

A gasless claim the server has already relayed succeeds instead of throwing.

The server rejects a second claim for a swap in `client_redeeming`,
`client_redeemed` or `server_redeemed` with "wrong state". `claim` now returns
`success: true` with that state, since the claim is done from the client's
side; any other rejection still throws.
