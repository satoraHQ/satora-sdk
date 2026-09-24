---
"@lendasat/lendaswap-sdk-pure": patch
---

Gasless funding (`fundSwapGasless`, `getCoordinatorFundingCallDataPermit2`) now works after the same SDK key has claimed a swap via the sponsored UserOp path. The claim delegates the SDK's EVM address to Kernel V3.3 through EIP-7702, after which Permit2 verifies signatures via Kernel's `isValidSignature`. The SDK now detects the delegation from the server's new `depositor_delegation` hint on the calldata endpoint (authoritative for the swap's chain; against older servers it falls back to probing the AA RPC on Arbitrum) and signs both the Permit2 message and the EIP-2612 permit in Kernel's ERC-1271 envelope instead of as a plain EOA (the permit is sent as an opaque `signature` and submitted through the token's bytes-signature `permit` variant). Helpers `kernelErc1271Digest`, `wrapKernelErc1271Signature`, `isKernelDelegation` and `parseEip7702Delegation` are exported.
