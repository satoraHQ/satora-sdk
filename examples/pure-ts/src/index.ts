#!/usr/bin/env tsx
/**
 * Lendaswap Pure TypeScript SDK - CLI Example
 *
 * This CLI demonstrates how to use the Lendaswap Pure TypeScript SDK
 * with SQLite storage. For browser apps, use IdbWalletStorage instead.
 *
 * Usage:
 *   tsx src/index.ts pairs                          - List available trading pairs
 *   tsx src/index.ts quote <from> <to> <amount>     - Get a quote
 *   tsx src/index.ts swap <from> <to> <amount> [address] [evmAddr] - Create a swap
 *   tsx src/index.ts watch <id>                     - Watch swap status
 *   tsx src/index.ts redeem <id> [destination]      - Redeem a swap
 *   tsx src/index.ts refund <id>                    - Refund a swap
 *   tsx src/index.ts swaps                          - List stored swaps
 *   tsx src/index.ts status                         - Show detailed API status
 *   tsx src/index.ts info                           - Show wallet info
 *
 * Swap Directions:
 *   BTC to EVM:        btc_lightning/btc_arkade/btc_onchain -> usdc_pol/usdc_arb/usdc_eth
 *   BTC to Arkade:     btc_onchain -> btc_arkade
 *   EVM to Arkade:     usdc_pol/usdc_arb/usdc_eth -> btc_arkade (requires evmAddr)
 */

// Load .env file before anything else
import "dotenv/config";

import { Client } from "@lendasat/lendaswap-sdk-pure";
import { sqliteStorageFactory } from "@lendasat/lendaswap-sdk-pure/node";
import * as path from "node:path";
import * as os from "node:os";

import { listPairs } from "./commands/pairs.js";
import { getQuote } from "./commands/quote.js";
import { createSwap } from "./commands/swap.js";
import { listSwaps } from "./commands/swaps.js";
import { showInfo } from "./commands/info.js";
import { watchSwap } from "./commands/watch.js";
import { redeemSwap } from "./commands/redeem.js";
import { refundSwap } from "./commands/refund.js";
import { evmFundSwap } from "./commands/evm-fund.js";
import { evmFundPermit2 } from "./commands/evm-fund-permit2.js";
import { fundGasless } from "./commands/fund-gasless.js";
import { evmRefundSwap } from "./commands/evm-refund.js";
import { evmClaimSwap } from "./commands/evm-claim.js";
import { showEvmBalances } from "./commands/evm-balances.js";
import { recoverSwaps } from "./commands/recover.js";
import { showStatus } from "./commands/status.js";
import { delegateSettle } from "./commands/delegate-settle.js";
import { deriveSwapEvmAddress } from "./commands/derive-evm-address.js";

// Configuration from environment variables
export const CONFIG = {
  apiUrl: process.env.LENDASWAP_API_URL || "https://api.satora.io/",
  mnemonic: process.env.MNEMONIC,
  evmMnemonic: process.env.EVM_MNEMONIC, // Separate mnemonic for EVM wallet
  orgCode: process.env.LENDASWAP_ORG_CODE,
  dbPath:
    process.env.LENDASWAP_DB_PATH ||
    path.join(os.homedir(), ".lendaswap", "data.db"),
  esploraUrl: process.env.ESPLORA_URL, // Optional, defaults by network
  arkadeUrl: process.env.ARKADE_URL, // Optional, for regtest/custom Arkade servers
};

// Ensure the database directory exists
import * as fs from "node:fs";

const dbDir = path.dirname(CONFIG.dbPath);
if (!fs.existsSync(dbDir)) {
  fs.mkdirSync(dbDir, { recursive: true });
}

// SQLite storage (persists to disk)
const {
  walletStorage,
  swapStorage,
  close: closeStorage,
} = sqliteStorageFactory(CONFIG.dbPath);

export { swapStorage };

/**
 * Create and initialize the client.
 */
async function createClient(): Promise<Client> {
  let builder = Client.builder()
    .withBaseUrl(CONFIG.apiUrl)
    .withSignerStorage(walletStorage)
    .withSwapStorage(swapStorage);

  if (CONFIG.orgCode) {
    builder = builder.withOrgCode(CONFIG.orgCode);
  }

  if (CONFIG.mnemonic) {
    builder = builder.withMnemonic(CONFIG.mnemonic);
  }

  if (CONFIG.esploraUrl) {
    builder = builder.withEsploraUrl(CONFIG.esploraUrl);
  }

  if (CONFIG.arkadeUrl) {
    builder = builder.withArkadeServerUrl(CONFIG.arkadeUrl);
  }

  return builder.build();
}

function showHelp(): void {
  console.log(`
Lendaswap CLI - Pure TypeScript SDK Example

Usage:
  tsx src/index.ts <command> [options]

Commands:
  pairs                              List available trading pairs
  quote <from> <to> <amount>         Get a quote for a swap
  swap <from> <to> <amount> <addr>   Create a new swap
  evm-fund <id>                      Fund an EVM HTLC (EVM-to-Arkade/Lightning)
  evm-fund-permit2 <id>              Fund via Permit2 (gasless signing)
  fund-gasless <id>                  Fund via gasless relay (no wallet/ETH needed)
  watch <id>                         Watch a swap's status (polls backend)
  redeem <id> [destination]          Redeem a swap (when serverfunded)
  refund <id> [addr] [fee]           Refund a swap (addr/fee for on-chain, --collaborative for EVM collab refund)
  evm-refund <id> [--direct] [--force] Refund EVM HTLC (--direct: WBTC, --force: skip timelock check)
  evm-claim <id>                     Claim EVM tokens (BTC-to-Ethereum only)
  evm-balances                       Show EVM wallet balances (all chains)
  derive-evm-address <id>            Derive EVM address from stored swap key
  swaps                              List locally stored swaps
  recover                            Recover swaps from server
  status                             Show detailed API and dependency status
  info                               Show wallet info
  help                               Show this help message

Examples (Arkade to EVM — gasless, no address needed):
  tsx src/index.ts swap btc_arkade usdc_pol 100000
  tsx src/index.ts swap btc_arkade usdc_arb 100000

Examples (BTC to EVM):
  tsx src/index.ts swap btc_lightning usdc_pol 100000 0x1234...
  tsx src/index.ts swap btc_onchain usdc_pol 100000 0x1234...

Examples (BTC on-chain to Arkade):
  tsx src/index.ts swap btc_onchain btc_arkade 100000 ark1...

Examples (EVM to Arkade):
  tsx src/index.ts swap usdc_pol btc_arkade 100 ark1... 0x1234...
  tsx src/index.ts swap usdc_arb btc_arkade 100 ark1... 0x1234...

Examples (EVM to Lightning):
  tsx src/index.ts swap usdc_pol btc_lightning lnbc... 0x1234...
  tsx src/index.ts swap usdc_arb btc_lightning lnbc... 0x1234...

Other Examples:
  tsx src/index.ts pairs
  tsx src/index.ts quote btc_lightning usdc_pol 100000
  tsx src/index.ts watch 12345678-1234-1234-1234-123456789abc
  tsx src/index.ts redeem 12345678-1234-1234-1234-123456789abc
  tsx src/index.ts redeem 12345678-... 0x1234...   (Arkade-to-EVM gasless claim)
  tsx src/index.ts refund 12345678-... bc1q... 5
  tsx src/index.ts swaps
  tsx src/index.ts status
  tsx src/index.ts info

Environment Variables:
  LENDASWAP_API_URL   API URL (default: https://api.satora.io/)
  MNEMONIC            Wallet mnemonic for BTC operations (optional, generates new if not set)
  EVM_MNEMONIC        Wallet mnemonic for EVM operations (required for fund command)
  LENDASWAP_ORG_CODE  Org code for swap tracking (optional)
  LENDASWAP_DB_PATH   SQLite database path (default: ~/.lendaswap/data.db)
  ESPLORA_URL         Esplora API URL for broadcasting (default: mempool.space)
`);
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const command = args[0];

  if (
    !command ||
    command === "help" ||
    command === "--help" ||
    command === "-h"
  ) {
    showHelp();
    return;
  }

  // Commands that don't need the API client
  if (command === "derive-evm-address") {
    await deriveSwapEvmAddress(swapStorage, args[1]);
    return;
  }

  const client = await createClient();

  switch (command) {
    case "pairs":
      await listPairs(client);
      break;
    case "quote":
      await getQuote(client, args[1], args[2], args[3]);
      break;
    case "swap": {
      const gaslessFlag = process.argv.includes("--gasless");
      await createSwap(
        client,
        args[1],
        args[2],
        args[3],
        args[4],
        CONFIG.evmMnemonic,
        gaslessFlag,
      );
      break;
    }
    case "evm-fund":
      await evmFundSwap(client, swapStorage, args[1], CONFIG.evmMnemonic);
      break;
    case "evm-fund-permit2":
      await evmFundPermit2(client, swapStorage, args[1], CONFIG.evmMnemonic);
      break;
    case "fund-gasless":
      await fundGasless(client, swapStorage, args[1]);
      break;
    case "watch":
      await watchSwap(client, args[1]);
      break;
    case "redeem":
      await redeemSwap(client, swapStorage, args[1], args[2]);
      break;
    case "refund": {
      const collaborativeFlag = args.includes("--collaborative");
      const directFlag = args.includes("--direct");
      const settlement = directFlag
        ? ("direct" as const)
        : ("swap-back" as const);
      // Filter out flags to get positional args
      const refundArgs = args.filter((a, i) => i > 0 && !a.startsWith("--"));
      await refundSwap(
        client,
        swapStorage,
        refundArgs[0],
        refundArgs[1],
        refundArgs[2],
        refundArgs[3],
        collaborativeFlag,
        settlement,
      );
      break;
    }
    case "evm-refund": {
      // Parse --direct and --force flags from any position
      const directMode = args.includes("--direct");
      const forceMode = args.includes("--force");
      const swapIdArg = args.find((a, i) => i > 0 && !a.startsWith("--"));
      await evmRefundSwap(
        client,
        swapIdArg,
        CONFIG.evmMnemonic,
        directMode,
        forceMode,
      );
      break;
    }
    case "evm-claim":
      await evmClaimSwap(client, args[1], CONFIG.evmMnemonic);
      break;
    case "evm-balances":
      await showEvmBalances(CONFIG.evmMnemonic);
      break;
    case "swaps":
      await listSwaps(swapStorage);
      break;
    case "recover":
      await recoverSwaps(client);
      break;
    case "status":
      await showStatus(client);
      break;
    case "delegate-settle":
      await delegateSettle(client, swapStorage, args[1], args[2]);
      break;
    case "info":
      await showInfo(client, CONFIG);
      break;
    default:
      console.error(`Unknown command: ${command}`);
      console.error("Run 'tsx src/index.ts help' for usage information.");
      process.exit(1);
  }
}

main()
  .catch((error) => {
    console.error("Error:", error.message);
    process.exit(1);
  })
  .finally(() => {
    closeStorage();
  });
