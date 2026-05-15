# Lendaswap Pure TypeScript SDK - CLI Example

A simple CLI demonstrating the Lendaswap Pure TypeScript SDK.

## Setup

```bash
npm install

# Optional: Create a .env file from the example
cp .env.example .env
```

## Usage

```bash
# Show help
npm start -- help

# List available trading pairs
npm run pairs

# Get a quote
npm run quote -- btc_lightning usdc_pol 100000

# Create a swap (BTC to EVM)
npm run swap -- btc_lightning usdc_pol 100000 0xYourAddress
npm run swap -- btc_arkade usdc_arb 100000 0xYourAddress
npm run swap -- btc_onchain usdc_eth 100000 0xYourAddress

# Create a swap (EVM to Arkade/Lightning)
npm run swap -- usdc_pol btc_arkade 100 ark1... 0xYourEvmAddress
npm run swap -- usdc_arb btc_lightning lnbc... 0xYourEvmAddress

# Fund EVM HTLC (for EVM-to-BTC swaps)
npm run evm-fund -- <swap-id>

# Watch a swap's status (polls every 5 seconds)
npm run watch -- <swap-id>

# Redeem a swap (when in serverfunded status)
npm run redeem -- <swap-id>

# Refund a swap (BTC on-chain swaps)
npm run refund -- <swap-id>                           # Check refund status
npm run refund -- <swap-id> <btc-address> 5           # Refund with 5 sat/vB fee
npm run refund -- <swap-id> <btc-address> 5 --dry-run # Build tx without broadcasting

# Refund an EVM HTLC (for EVM-to-BTC swaps after timelock)
npm run evm-refund -- <swap-id>

# Claim EVM tokens (BTC-to-Ethereum only)
npm run evm-claim -- <swap-id>

# Show EVM wallet balances across all chains
npm run evm-balances

# List locally stored swaps
npm run swaps

# Show detailed API and dependency status
npm run status

# Show wallet and API info
npm run info
```

## Environment Variables

| Variable             | Description                                          | Default                      |
| -------------------- | ---------------------------------------------------- | ---------------------------- |
| `LENDASWAP_API_URL`  | API base URL                                         | `https://api.lendaswap.com/` |
| `MNEMONIC`           | BIP39 mnemonic for BTC wallet (auto-generated)       | -                            |
| `EVM_MNEMONIC`       | BIP39 mnemonic for EVM wallet (required for EVM ops) | -                            |
| `LENDASWAP_ORG_CODE` | Org code for swap tracking                           | -                            |
| `LENDASWAP_DB_PATH`  | SQLite database path                                 | `~/.lendaswap/data.db`       |
| `ESPLORA_URL`        | Esplora API URL for broadcasting                     | `https://mempool.space/api`  |

## Example with Custom Mnemonic

```bash
# BTC wallet mnemonic
MNEMONIC="your twelve word mnemonic phrase here" npm run pairs

# EVM wallet mnemonic (for EVM-to-BTC swaps)
EVM_MNEMONIC="your twelve word mnemonic phrase here" npm run evm-balances
```

## Website Documentation Examples

Standalone scripts mirroring the code snippets from the [LendaSwap docs](https://lendasat.com/docs/lendaswap). Each file is runnable individually and uses SQLite storage instead of the browser-specific `IdbWalletStorage`/`IdbSwapStorage` shown in the docs.

```bash
# Setup & initialization (version check, mnemonic, xpub)
npm run docs:setup

# Quotes, price feed, price calculations, asset pairs
npm run docs:quotes

# BTC -> EVM swaps (Lightning & Arkade to Polygon/Ethereum)
npm run docs:create-btc-to-evm

# EVM -> BTC swaps (Polygon/Ethereum to Arkade/Lightning)
npm run docs:create-evm-to-btc

# On-chain BTC -> Arkade swaps
npm run docs:create-onchain-to-arkade

# Monitor swaps (get by ID, poll, list, filter, delete)
npm run docs:monitor-swaps
npm run docs:monitor-swaps -- <swap-id>

# Claim swaps (VHTLC, manual EVM)
npm run docs:claim -- <swap-id>

# Refund swaps (VHTLC, EVM HTLC, on-chain)
npm run docs:refund -- <swap-id>

# Error handling patterns
npm run docs:error-handling

# Recovery from seed + state machine
npm run docs:recovery
```

## Notes

- This example uses SQLite storage for persistent data (wallet mnemonic, key index, and swaps)
- Data is stored in `~/.lendaswap/data.db` by default (can be changed via `LENDASWAP_DB_PATH`)
- For browser apps, use `IdbWalletStorage` and `IdbSwapStorage` for persistent IndexedDB storage
- For React Native, implement custom storage backends using `WalletStorage` and `SwapStorage` interfaces
