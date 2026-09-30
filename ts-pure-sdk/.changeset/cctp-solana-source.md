---
"@lendasat/lendaswap-sdk-pure": minor
---

Send USDC from Solana to Bitcoin, Lightning or Arkade through CCTP. `getQuote`
and `createSwap` accept `{ chain: "Solana", tokenId: <USDC mint> }` as the
source (swaps must be created `gasless: true`), and the new
`fundSwapFromSolana(swapId, solanaSigner, options)` burns the USDC from any
Solana wallet wrapped as a `SolanaSigner`, waits for Circle's attestation and
settles on Arbitrum with the SDK's key in one sponsored UserOp. The burn
signature is recorded like an EVM burn, so a second call resumes instead of
burning twice and `recoverCctpInboundSwap` works for Solana burns too. Set the
Solana RPC with `withSolanaRpcUrl(url)` on the builder (or per call);
`getSolanaUsdcBalance(owner)` reads the wallet's USDC. The Solana code loads on
demand, so EVM-only consumers don't pay for it.
