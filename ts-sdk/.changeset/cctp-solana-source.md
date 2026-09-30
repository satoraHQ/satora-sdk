---
"@satora/swap": minor
---

Send USDC from Solana: `fundSwapFromSolana` burns USDC from a Solana wallet and
settles the swap on Arbitrum, `getSolanaUsdcBalance` reads the wallet's USDC,
and the builder gains `withSolanaRpcUrl` (see the legacy SDK changeset).
