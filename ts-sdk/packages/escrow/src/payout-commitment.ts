/**
 * Buyer's commitment to a payout Ark address.
 *
 * The buyer's destination is an Ark address (bech32m wrapping a server
 * pubkey + vtxo taproot key), not an L1 P2WPKH/P2TR address, so an L1
 * BIP-322 flow does not apply directly. Instead the buyer Schnorr-signs a
 * deterministic message tying the offer, the destination address, and the
 * x-only pubkey embedded in that destination — binding them to the payout
 * destination used in the cooperative release.
 */
export function payoutCommitmentMessage(
  offerId: string,
  payoutArkAddress: string,
): string {
  return `escrow:take:${offerId}:${payoutArkAddress}`;
}
