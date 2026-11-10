/**
 * Checks that what the server says about a swap binds to what this client
 * holds, before the client stores it, shows it or pays into it.
 */

import { ripemd160 } from "@noble/hashes/legacy.js";
import { hex } from "@scure/base";
import type {
  ArkadeToLightningSwapResponse,
  BitcoinToLightningSwapResponse,
  GetSwapResponse,
} from "./api/client.js";
import { getNetworkHrp, getNetworkName } from "./arkade-network.js";
import { invoiceAmountMsat, invoicePaymentHash } from "./bolt11.js";
import { parseXOnlyPubKey } from "./refund/arkade.js";
import { type BitcoinNetwork, verifyHtlcAddress } from "./refund/onchain.js";
import { bytesToHex, type SwapParams } from "./signer/index.js";
import type { LightningPaymentHash, StoredSwap } from "./storage/types.js";
import {
  ARKADE_HTLC_SCRIPT_VERSION_STRICT,
  BTC_HTLC_SCRIPT_VERSION_STRICT,
  StrictVhtlcScript,
} from "./strict-vhtlc.js";

/**
 * The server's terms for a swap do not bind to what this client holds: a
 * Lightning invoice that does not pay the swap's hash lock, a lock that does
 * not open with it, or terms that changed after the swap was created. Do not
 * pay or fund the swap.
 */
export class SwapVerificationError extends Error {
  /** The response field that failed the check, e.g. `bolt11_invoice`. */
  readonly field: string;
  /** What this client expected the field to commit to. */
  readonly expected: string;
  /** What the server's field commits to. */
  readonly received: string;
  readonly swapId?: string;

  constructor(
    message: string,
    details: {
      field: string;
      expected: string;
      received: string;
      swapId?: string;
    },
  ) {
    super(message);
    this.name = "SwapVerificationError";
    this.field = details.field;
    this.expected = details.expected;
    this.received = details.received;
    this.swapId = details.swapId;
  }
}

/** A hex hash without `0x`, lowercase. */
export function normalizeHash(hash: string): string {
  return hash.toLowerCase().replace(/^0x/, "");
}

/** An invoice's payment hash as lowercase hex without `0x`, like {@link normalizeHash}. */
function paymentHashOf(invoice: string): string {
  return normalizeHash(invoicePaymentHash(invoice));
}

/**
 * Checks that the invoice the payer will pay locks to `hashLock`, this
 * client's own preimage hash. A Lightning to X swap's server could otherwise
 * settle the payment with a preimage it chose and never fund the other side.
 */
export function assertInvoicePays(invoice: string, hashLock: string): void {
  const paymentHash = paymentHashOf(invoice);
  const expected = normalizeHash(hashLock);
  if (paymentHash !== expected) {
    throw new SwapVerificationError(
      `The swap's Lightning invoice pays hash ${paymentHash}, not this swap's hash lock ${expected}; do not pay it`,
      { field: "bolt11_invoice", expected, received: paymentHash },
    );
  }
}

/**
 * The payment hash an X to Lightning swap's lock must use, checked against
 * the server's answer. From an invoice the caller supplied it is read off
 * that invoice, and the server must pay that invoice and lock to its hash.
 * For a Lightning address or LNURL the server resolves the invoice, so the
 * hash is read off the invoice it returned, which must carry its `hash_lock`
 * and the payout it quoted (and the payout asked for, if one was): what
 * remains trusted is that the server resolved the address honestly, which
 * the client cannot check without resolving it itself.
 *
 * @throws {SwapVerificationError} if any of that fails; do not fund the swap.
 */
export function checkedLightningPaymentHash(
  options: { lightningInvoice?: string; targetAmountSats?: number },
  data: {
    hash_lock: string;
    client_lightning_invoice?: string | null;
    target_amount?: string | number | null;
  },
): LightningPaymentHash {
  const lock = normalizeHash(data.hash_lock);
  const returned = data.client_lightning_invoice
    ? paymentHashOf(data.client_lightning_invoice)
    : undefined;
  if (options.lightningInvoice !== undefined) {
    const hash = paymentHashOf(options.lightningInvoice);
    if (returned !== undefined && returned !== hash) {
      throw new SwapVerificationError(
        `The server returned an invoice for hash ${returned}, not the supplied invoice's ${hash}; do not fund the swap`,
        {
          field: "client_lightning_invoice",
          expected: hash,
          received: returned,
        },
      );
    }
    if (lock !== hash) {
      throw new SwapVerificationError(
        `The server created the swap under hash ${lock}, not the invoice's payment hash ${hash}; do not fund it`,
        { field: "hash_lock", expected: hash, received: lock },
      );
    }
    return { hash: `0x${hash}`, source: "invoice" };
  }
  if (!data.client_lightning_invoice || returned === undefined) {
    throw new SwapVerificationError(
      "The server returned no invoice for this Lightning address, so the swap's hash lock cannot be checked; do not fund it",
      {
        field: "client_lightning_invoice",
        expected: "an invoice",
        received: "",
      },
    );
  }
  if (lock !== returned) {
    throw new SwapVerificationError(
      `The server created the swap under hash ${lock}, not the payment hash ${returned} of the invoice it resolved; do not fund it`,
      { field: "hash_lock", expected: returned, received: lock },
    );
  }
  const msat = invoiceAmountMsat(data.client_lightning_invoice);
  const asked = [data.target_amount, options.targetAmountSats].filter(
    (amount) => amount !== undefined && amount !== null,
  );
  for (const sats of asked) {
    if (
      msat === undefined ||
      msat !== BigInt(sats as string | number) * 1000n
    ) {
      throw new SwapVerificationError(
        `The invoice the server resolved asks for ${msat === undefined ? "any amount" : `${msat} msat`}, not the ${sats} sats payout; do not fund it`,
        {
          field: "client_lightning_invoice",
          expected: `${sats} sats`,
          received: msat === undefined ? "any amount" : `${msat} msat`,
        },
      );
    }
  }
  return { hash: `0x${returned}`, source: "server-invoice" };
}

/**
 * Rebuilds the Arkade to Lightning VHTLC the user is asked to fund, as a
 * refund rebuilds it, from the invoice's payment hash and the swap's refund
 * key, and refuses a script version this client cannot refund. Consumers fund
 * `arkade_vhtlc_address` straight from the stored response, so it is checked
 * before the swap is first stored: at creation, or when recovery stores it.
 */
export function assertArkadeToLightningVhtlc(
  response: ArkadeToLightningSwapResponse,
  paymentHash: string,
  refundPublicKey: Uint8Array,
): void {
  const swapId = response.id;
  if (
    response.arkade_htlc_script_version !== ARKADE_HTLC_SCRIPT_VERSION_STRICT
  ) {
    throw new SwapVerificationError(
      `Swap ${swapId} uses Arkade HTLC script version ${response.arkade_htlc_script_version}, which this client cannot refund; do not fund it`,
      {
        field: "arkade_htlc_script_version",
        expected: String(ARKADE_HTLC_SCRIPT_VERSION_STRICT),
        received: String(response.arkade_htlc_script_version),
        swapId,
      },
    );
  }
  const seconds = (value: number) => ({
    type: "seconds" as const,
    value: BigInt(value),
  });
  const server = parseXOnlyPubKey(response.arkade_server_pk);
  const vhtlc = new StrictVhtlcScript({
    sender: parseXOnlyPubKey(bytesToHex(refundPublicKey)),
    receiver: parseXOnlyPubKey(response.receiver_pk),
    server,
    preimageHash: ripemd160(hex.decode(paymentHash)),
    refundLocktime: BigInt(response.vhtlc_refund_locktime),
    unilateralClaimDelay: seconds(response.unilateral_claim_delay),
    unilateralRefundDelay: seconds(response.unilateral_refund_delay),
    unilateralRefundWithoutReceiverDelay: seconds(
      response.unilateral_refund_without_receiver_delay,
    ),
  });
  const expected = vhtlc
    .address(getNetworkHrp(getNetworkName(response.network)), server)
    .encode();
  if (expected !== response.arkade_vhtlc_address) {
    throw new SwapVerificationError(
      `Swap ${swapId}'s VHTLC address ${response.arkade_vhtlc_address} does not lock to the invoice's payment hash and this client's refund key (expected ${expected}); do not fund it`,
      {
        field: "arkade_vhtlc_address",
        expected,
        received: response.arkade_vhtlc_address,
        swapId,
      },
    );
  }
}

/**
 * Rebuilds the Bitcoin to Lightning HTLC the user is asked to fund, as a
 * refund rebuilds it, from the invoice's payment hash and the swap's refund
 * key. The refund rebuilds it from `btc_hash_lock`, so that must be the
 * invoice's hash too, or the HTLC could be funded and never refunded. Checked
 * before the swap is first stored, like the Arkade to Lightning VHTLC.
 */
export function assertBitcoinToLightningHtlc(
  response: BitcoinToLightningSwapResponse,
  paymentHash: string,
  refundPublicKey: Uint8Array,
): void {
  const swapId = response.id;
  if (response.btc_htlc_script_version !== BTC_HTLC_SCRIPT_VERSION_STRICT) {
    throw new SwapVerificationError(
      `Swap ${swapId} uses Bitcoin HTLC script version ${response.btc_htlc_script_version}, which this client cannot refund; do not fund it`,
      {
        field: "btc_htlc_script_version",
        expected: String(BTC_HTLC_SCRIPT_VERSION_STRICT),
        received: String(response.btc_htlc_script_version),
        swapId,
      },
    );
  }
  const hash160 = hex.encode(ripemd160(hex.decode(paymentHash)));
  const serverHash160 = normalizeHash(response.btc_hash_lock);
  if (serverHash160 !== hash160) {
    throw new SwapVerificationError(
      `Swap ${swapId}'s HTLC hash ${serverHash160} is not the HASH160 ${hash160} of the invoice's payment hash; do not fund it`,
      {
        field: "btc_hash_lock",
        expected: hash160,
        received: serverHash160,
        swapId,
      },
    );
  }
  const name = getNetworkName(response.network);
  const network: BitcoinNetwork =
    name === "bitcoin" ? "mainnet" : name === "mutinynet" ? "signet" : name;
  const binds = verifyHtlcAddress(
    response.btc_htlc_address,
    hash160,
    bytesToHex(parseXOnlyPubKey(response.btc_server_pk)),
    bytesToHex(parseXOnlyPubKey(bytesToHex(refundPublicKey))),
    response.btc_refund_locktime,
    network,
    response.btc_htlc_script_version,
  );
  if (!binds) {
    throw new SwapVerificationError(
      `Swap ${swapId}'s HTLC address ${response.btc_htlc_address} does not lock to the invoice's payment hash and this client's refund key; do not fund it`,
      {
        field: "btc_htlc_address",
        expected:
          "an HTLC on the invoice's payment hash and this client's refund key",
        received: response.btc_htlc_address,
        swapId,
      },
    );
  }
}

/**
 * How the keys recovery derives at the server's `index` fail to match a
 * stored swap, if they do. The swap's locks were built on the stored keys and
 * preimage, so keys from another index must never replace them.
 */
export function recoveredKeysChange(
  stored: StoredSwap,
  index: number,
  derivedPreimageHash: string,
): SwapVerificationError | undefined {
  if (
    normalizeHash(derivedPreimageHash) === normalizeHash(stored.preimageHash)
  ) {
    return undefined;
  }
  return new SwapVerificationError(
    `The server's index ${index} for swap ${stored.swapId} derives other keys than the stored index ${stored.keyIndex}; keeping the stored copy`,
    {
      field: "index",
      expected: String(stored.keyIndex),
      received: String(index),
      swapId: stored.swapId,
    },
  );
}

/**
 * How a server copy of a swap this client has not stored fails the checks
 * creation makes on the server's terms, if it does: a Lightning to X invoice
 * must pay the hash of the secret in `swapParams`, and an Arkade or Bitcoin
 * to Lightning hash lock must be the payment hash of an invoice for the
 * swap's payout, with a lock that rebuilds from that hash and the refund key
 * in `swapParams`. A field that is missing or does not decode fails too.
 */
export function unboundTerms(
  fresh: GetSwapResponse,
  swapParams: SwapParams,
): Error | undefined {
  try {
    switch (fresh.direction) {
      case "lightning_to_arkade":
      case "lightning_to_evm":
      case "lightning_to_bitcoin":
        assertInvoicePays(
          fresh.bolt11_invoice,
          bytesToHex(swapParams.preimageHash),
        );
        break;
      // EVM-sourced swaps are left to the commitments their creation records.
      case "arkade_to_lightning":
        assertArkadeToLightningVhtlc(
          fresh,
          normalizeHash(checkedLightningPaymentHash({}, fresh).hash),
          swapParams.publicKey,
        );
        break;
      case "bitcoin_to_lightning":
        assertBitcoinToLightningHtlc(
          fresh,
          normalizeHash(checkedLightningPaymentHash({}, fresh).hash),
          swapParams.publicKey,
        );
        break;
    }
  } catch (error) {
    return error instanceof Error ? error : new Error(String(error));
  }
  return undefined;
}

const EVM_SOURCED = new Set([
  "evm_to_bitcoin",
  "evm_to_arkade",
  "evm_to_lightning",
]);
const HASH_FIELDS = ["hash_lock", "evm_hash_lock", "btc_hash_lock"];
const ADDRESS_FIELDS = [
  "arkade_vhtlc_address",
  "btc_vhtlc_address",
  "btc_htlc_address",
  "evm_htlc_address",
];
const INVOICE_FIELDS = ["bolt11_invoice", "client_lightning_invoice"];

/**
 * An invoice compared by the hash it pays. One that does not decode is
 * compared as is, so a record stored before invoices were checked still
 * matches an unchanged copy.
 */
function invoiceKey(invoice: string): string {
  try {
    return paymentHashOf(invoice);
  } catch {
    return invoice;
  }
}

/**
 * How a fresh server copy of a stored swap fails to bind to it, if it does:
 * a direction, hash lock, lock address or invoice payment hash that differs
 * from the stored one, a Lightning to X invoice that does not pay this client's
 * preimage hash, or an X to Lightning hash lock that is not its invoice's
 * payment hash. These never change once a swap exists, so a difference means
 * the copy must not replace the stored one.
 *
 * EVM-sourced swaps are not checked: their funding uses what creation
 * recorded (`lightningPaymentHash`, `evmFundingTerms`), never the server's
 * copy, and a rehash legitimately moves their hash lock and invoice.
 */
export function swapTermsChange(
  stored: StoredSwap,
  fresh: GetSwapResponse,
): SwapVerificationError | undefined {
  if (EVM_SOURCED.has(stored.response.direction)) return undefined;
  const before = stored.response as unknown as Record<string, unknown>;
  const after = fresh as unknown as Record<string, unknown>;
  const swapId = stored.swapId;
  const changed = (field: string, expected: string, received: string) =>
    new SwapVerificationError(
      `The server's ${field} for swap ${swapId} commits to ${received}, expected ${expected}; keeping the stored copy`,
      { field, expected, received, swapId },
    );

  const compare = (fields: string[], key: (value: string) => string) => {
    for (const field of fields) {
      const was = before[field];
      if (typeof was !== "string" || was === "") continue;
      const now = after[field];
      const expected = key(was);
      const received = typeof now === "string" ? key(now) : String(now);
      if (received !== expected) return changed(field, expected, received);
    }
    return undefined;
  };
  const difference =
    compare(["direction"], (direction) => direction) ??
    compare(HASH_FIELDS, normalizeHash) ??
    compare(ADDRESS_FIELDS, (address) => address.toLowerCase()) ??
    compare(INVOICE_FIELDS, invoiceKey);
  if (difference) return difference;

  if (typeof after.bolt11_invoice === "string") {
    const expected = normalizeHash(stored.preimageHash);
    const received = invoiceKey(after.bolt11_invoice);
    if (received !== expected) {
      return changed("bolt11_invoice", expected, received);
    }
  }
  if (
    typeof after.client_lightning_invoice === "string" &&
    typeof after.hash_lock === "string"
  ) {
    const expected = invoiceKey(after.client_lightning_invoice);
    const received = normalizeHash(after.hash_lock);
    if (received !== expected) return changed("hash_lock", expected, received);
  }
  return undefined;
}
