/**
 * EVM to Lightning swap creation.
 *
 * The user locks an ERC-20 token on an EVM chain in an HTLCErc20 (via the
 * coordinator) whose hash lock is the payment hash of the Lightning
 * invoice they want paid; the server pays the invoice and claims the HTLC
 * with the revealed preimage.
 */

import { invoiceAmountMsat, invoicePaymentHash } from "../bolt11.js";
import { bytesToHex } from "../signer/index.js";
import type { LightningPaymentHash } from "../storage/types.js";
import { fundingTermsOf } from "./funding-terms.js";
import type {
  CreateSwapContext,
  EvmToLightningSwapOptions,
  EvmToLightningSwapResult,
} from "./types.js";

/**
 * Creates a new EVM to Lightning swap.
 *
 * Flow:
 * 1. User funds the HTLCErc20 (`fundSwap` / `fundSwapGasless`) with `source_amount` token units
 * 2. Once the funding is final the server pays the Lightning invoice; the settled payment reveals
 *    the preimage
 * 3. Server claims the HTLC with the preimage
 *
 * The destination is **one of** `lightningInvoice` (its amount pins the
 * payout), or `lightningAddress`/`lnurl` with one of `sourceAmount`
 * (send-max: fees deducted from the payout) or `targetAmountSats` (exact
 * payout: fees added on top).
 *
 * Like Arkade → Lightning, the swap's hash lock is the invoice's payment
 * hash — the derived swap key is the EVM depositor for gasless funding and
 * the refund signer, so a duplicate-hash 409 is not retried with a new
 * key: it means this exact invoice already has a swap.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails.
 */
export async function createEvmToLightningSwap(
  options: EvmToLightningSwapOptions,
  ctx: CreateSwapContext,
): Promise<EvmToLightningSwapResult> {
  const destinations = [
    options.lightningInvoice,
    options.lightningAddress,
    options.lnurl,
  ].filter((d) => d !== undefined);
  if (destinations.length !== 1) {
    throw new Error(
      "Provide exactly one of lightningInvoice, lightningAddress or lnurl",
    );
  }
  if (
    options.lightningInvoice === undefined &&
    (options.sourceAmount === undefined) ===
      (options.targetAmountSats === undefined)
  ) {
    throw new Error("Provide exactly one of sourceAmount or targetAmountSats");
  }
  if (!options.gasless && !options.userAddress) {
    throw new Error("userAddress is required unless the swap is gasless");
  }

  const swapParams = await ctx.deriveSwapParams();
  const userId = bytesToHex(swapParams.userId);
  const userAddress = options.gasless ? ctx.evmAddress : options.userAddress;

  const { data, error } = await ctx.apiClient.POST("/swap/evm/lightning", {
    body: {
      lightning_invoice: options.lightningInvoice,
      lightning_address: options.lightningAddress,
      lnurl: options.lnurl,
      evm_chain_id: options.evmChainId,
      token_address: options.tokenAddress,
      amount_in:
        options.sourceAmount === undefined
          ? undefined
          : options.sourceAmount.toString(),
      target_amount_sats: options.targetAmountSats,
      user_address: userAddress ?? "",
      user_id: userId,
      referral_code: options.referralCode,
      extra_fees: options.extraFees,
      gasless: options.gasless ?? false,
      bridge_source_chain: options.inboundBridgeParams?.sourceChain,
      bridge_source_token_address:
        options.inboundBridgeParams?.sourceTokenAddress,
    },
  });
  if (error) {
    throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
  }
  if (!data) {
    throw new Error("No swap data returned");
  }

  // The lock's hash decides who can claim it: one the server knows the
  // preimage of lets it claim without paying. It must be the invoice's.
  const lightningPaymentHash = checkedPaymentHash(options, data);

  // Store the swap if storage is configured — the derived key is the
  // gasless depositor and what signs a collaborative refund of the HTLC.
  await ctx.storeSwap(
    data.id,
    swapParams,
    {
      ...data,
      direction: "evm_to_lightning",
    },
    undefined,
    undefined,
    { lightningPaymentHash, evmFundingTerms: fundingTermsOf(data) },
  );

  return { response: data, swapParams };
}

/**
 * The payment hash the swap's lock must use. From an invoice the caller
 * supplied it is read off that invoice. For a Lightning address or LNURL
 * the server resolves the invoice, so the hash is read off the invoice it
 * returned, which must carry its `hash_lock` and the payout it quoted (and
 * the payout asked for, if one was): what remains trusted is that the
 * server resolved the address honestly, which the client cannot check
 * without resolving it itself.
 */
function checkedPaymentHash(
  options: EvmToLightningSwapOptions,
  data: {
    hash_lock: string;
    client_lightning_invoice?: string;
    target_amount?: string | number;
  },
): LightningPaymentHash {
  const norm = (hash: string) => `0x${hash.replace(/^0x/i, "").toLowerCase()}`;
  if (options.lightningInvoice !== undefined) {
    const hash = invoicePaymentHash(options.lightningInvoice);
    if (norm(data.hash_lock) !== hash) {
      throw new Error(
        `The server created the swap under hash ${data.hash_lock}, not the invoice's payment hash ${hash}; do not fund it`,
      );
    }
    return { hash, source: "invoice" };
  }
  const invoice = data.client_lightning_invoice;
  if (!invoice) {
    throw new Error(
      "The server returned no invoice for this Lightning address, so the swap's hash lock cannot be checked; do not fund it",
    );
  }
  const hash = invoicePaymentHash(invoice);
  if (norm(data.hash_lock) !== hash) {
    throw new Error(
      `The server created the swap under hash ${data.hash_lock}, not the payment hash ${hash} of the invoice it resolved; do not fund it`,
    );
  }
  const msat = invoiceAmountMsat(invoice);
  const asked = [data.target_amount, options.targetAmountSats].filter(
    (amount) => amount !== undefined && amount !== null,
  );
  for (const sats of asked) {
    if (
      msat === undefined ||
      msat !== BigInt(sats as string | number) * 1000n
    ) {
      throw new Error(
        `The invoice the server resolved asks for ${msat === undefined ? "any amount" : `${msat} msat`}, not the ${sats} sats payout; do not fund it`,
      );
    }
  }
  return { hash, source: "server-invoice" };
}
