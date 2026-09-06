/**
 * Timeout-based refund: EVM → Arkade swap that expired.
 *
 * When an EVM-to-Arkade swap fails and collaborative refund is
 * unavailable (e.g. server is down), the user can still reclaim
 * their funds after the HTLC timelock expires by submitting the
 * refund transaction themselves via `refundEvmWithSigner`.
 */

import {
  Client,
  type EvmSigner,
  InMemorySwapStorage,
  InMemoryWalletStorage,
} from "../src/index.js";

// #region setup
const client = await Client.builder()
  .withSignerStorage(new InMemoryWalletStorage())
  .withSwapStorage(new InMemorySwapStorage())
  .withOrgCode(process.env.ORG_CODE || "")
  .build();
// #endregion setup

// Assume an EVM-to-Arkade swap was created and funded, but expired
const swapId = "550e8400-e29b-41d4-a716-446655440000";

// #region check-status
const swap = await client.getSwap(swapId);
console.log("Status:", swap.status);
// ... "expired"
console.log("Direction:", swap.direction);
// ... "evm_to_arkade"
// #endregion check-status

// #region timeout-refund
// Timeout-based refund: does NOT require server cooperation.
// The user must wait for the HTLC timelock to expire, then submit
// the refund transaction themselves with their EVM wallet.
//
// Build an EvmSigner from your wallet (see funding example for details)
declare const signer: EvmSigner; // your wallet — see EvmSigner docs

// refundEvmWithSigner handles everything: reads the locked amount off the
// funding tx's receipt (your waitForReceipt must return its logs), asks the
// server for calldata only if that fails, sends the transaction, and waits
// for the receipt.
// The locked WBTC/tBTC is returned directly to the depositor.
const { txHash } = await client.refundEvmWithSigner(swapId, signer);

console.log("Refund TX:", txHash);
// ... "0xabc123..."
// #endregion timeout-refund
