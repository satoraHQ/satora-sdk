---
"@satora/swap": minor
---

`fundSwapGasless` funds Arbitrum swaps itself, as one paymaster-sponsored UserOp from the depositor's own Kernel account (EIP-7702): `approve(Permit2)` + `executeAndCreateWithPermit2`, the mechanism the CCTP-inbound settlement and refund continuations already use. The server's legacy relay is no longer involved there, and a sponsorship or configuration failure surfaces as an error instead of falling back to it. The batch only approves Permit2 when the account's allowance requires it and resets a lingering smaller allowance to zero first, so tokens without EIP-2612 — including USDT-style tokens that revert a non-zero → non-zero approve — fund without a prior approval transaction. A funding this client already recorded is answered with its transaction instead of being submitted again. Chains without a sponsorship policy (Ethereum, Polygon) keep the relay, and legacy swaps whose depositor is the per-swap key are funded from that key's account.

New `gaslessFundingPlan(chainId)` names the funding path (`sponsored` on Arbitrum, `relay` elsewhere) and throws when the sponsored path applies but the client was built without `withAa(...)` — callable before a deposit address is shown, so no deposit is requested that could not be moved afterwards.

Integrators upgrading existing Arbitrum gasless flows must configure `withAa(...)` and call `gaslessFundingPlan(chainId)` before displaying a deposit address. The usdc2bitcoin, usdt2bitcoin and tbtc2bitcoin apps now withhold deposit instructions while this check is pending or fails; they no longer discover missing AA configuration only after a deposit arrives.

Arbitrum sponsored funding requires a nonempty `paymasterPolicyId` as well as AA
configuration. Tests using Alto without a paymaster must explicitly set
`allowSelfPaidFunding: true` and provide native ETH; the funding plan reports
`self-paid`, which deposit UIs reject.
