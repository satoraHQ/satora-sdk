/**
 * Recover swaps from the server using the stored mnemonic.
 */

import type { Client } from "@lendasat/lendaswap-sdk-pure";

export async function recoverSwaps(client: Client): Promise<void> {
  console.log("Recovering swaps from server...");
  console.log("=".repeat(60));
  console.log("");

  // #region recover-swaps
  // Recover all swaps from the server
  const recovery = await client.recoverAllSwaps();
  console.log(`Recovered ${recovery.swaps.length} swaps`);
  // ... "Recovered 3 swaps"

  if (!recovery.complete) {
    console.warn("Recovery stopped before completion:", recovery.errorMessage);
    console.warn("Successful scans:", recovery.scans);
    console.warn("Scanned until:", recovery.scannedUntil);
  }
  // #endregion recover-swaps

  console.log("");

  // #region process-recovered
  const swaps = await client.listAllSwaps();

  for (const stored of swaps) {
    const swap = stored.response;
    switch (swap.status) {
      case "serverfunded":
        console.log(`Swap ${stored.swapId}: Ready to claim!`);
        // ... "Swap 550e8400-...: Ready to claim!"
        await client.claim(stored.swapId);
        break;
      case "clientfundedserverrefunded":
        console.log(`Swap ${stored.swapId}: Needs refund`);
        // ... "Swap 661f9511-...: Needs refund"
        break;
      case "clientredeemed":
        console.log(`Swap ${stored.swapId}: Complete`);
        // ... "Swap 772a0622-...: Complete"
        break;
      default:
        console.log(`Swap ${stored.swapId}: ${swap.status}`);
    }
  }
  // #endregion process-recovered

  if (swaps.length === 0) {
    console.log("  No swaps found. Set MNEMONIC in .env to recover from an existing wallet.");
  }

  console.log("");
  console.log("=".repeat(60));
}
