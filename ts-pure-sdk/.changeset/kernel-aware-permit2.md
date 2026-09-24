---
"@lendasat/lendaswap-sdk-pure": patch
---

Gasless funding (`fundSwapGasless`, `getCoordinatorFundingCallDataPermit2`) now works after the same SDK key has claimed a swap via the sponsored UserOp path. The claim delegates the SDK's EVM address to Kernel V3.3 through EIP-7702, after which Permit2 verifies signatures via Kernel's `isValidSignature`. The SDK now detects the delegation on the AA chain (Arbitrum, via the AA `rpcUrl`/`bundlerUrl` when configured; a 7702 delegation is per chain, so other chains keep the plain signature) and signs both the Permit2 message and the EIP-2612 permit in Kernel's ERC-1271 envelope instead of as a plain EOA (the permit is sent as an opaque `signature` and submitted through the token's bytes-signature `permit` variant). Helpers `kernelErc1271Digest`, `wrapKernelErc1271Signature`, `isKernelDelegation` and `parseEip7702Delegation` are exported.
