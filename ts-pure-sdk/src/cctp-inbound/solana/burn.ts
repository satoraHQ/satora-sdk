/**
 * Solana source-chain burn for the CCTP-inbound flow.
 *
 * Builds a legacy Solana transaction calling CCTPv2
 * `TokenMessengerMinterV2.deposit_for_burn`, which burns the owner's USDC and
 * emits a CCTP message to Arbitrum with `mintRecipient` and
 * `destinationCaller` pinned to the SDK key's Kernel account there.
 *
 * Hand-rolled on the noble/scure primitives the SDK already ships instead of
 * `@solana/web3.js` + Anchor, which would add several hundred kB for one
 * instruction. Layouts follow Circle's IDL and program source
 * (github.com/circlefin/solana-cctp-contracts, `programs/v2/
 * token-messenger-minter-v2`, IDL `token_messenger_minter_v2.json` v0.2.0).
 *
 * The program stores the outgoing message in a fresh, non-PDA account
 * (`message_sent_event_data`) that must sign the transaction. The SDK
 * generates that keypair, signs with it here, and leaves the fee-payer
 * signature slot empty for the wallet.
 */

import { ed25519 } from "@noble/curves/ed25519.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { base58 } from "@scure/base";
import { FINALITY_FAST } from "../../cctp/constants.js";
import {
  SOLANA_MESSAGE_TRANSMITTER_V2,
  SOLANA_TOKEN_MESSENGER_MINTER_V2,
  SOLANA_USDC_MINT,
} from "./constants.js";

// ============================================================================
// Programs and accounts (Solana mainnet)
// ============================================================================

const TOKEN_PROGRAM_ID = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const ASSOCIATED_TOKEN_PROGRAM_ID =
  "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const SYSTEM_PROGRAM_ID = "11111111111111111111111111111111";
const COMPUTE_BUDGET_PROGRAM_ID = "ComputeBudget111111111111111111111111111111";

/** Anchor discriminator of `deposit_for_burn` (sha256("global:deposit_for_burn")[..8]). */
export const DEPOSIT_FOR_BURN_DISCRIMINATOR = Uint8Array.from([
  215, 60, 61, 46, 114, 55, 128, 176,
]);

/**
 * Compute-unit limit for the burn transaction. A mainnet simulation of this
 * exact transaction consumed ~42k CU (the burn, the CPI into the message
 * transmitter and the Anchor CPI event); the limit leaves over 2x headroom
 * while keeping the priority fee, which is charged per requested CU, small.
 */
export const SOLANA_BURN_COMPUTE_UNIT_LIMIT = 100_000;

// ============================================================================
// Byte helpers
// ============================================================================

const PDA_MARKER = new TextEncoder().encode("ProgramDerivedAddress");
const utf8 = (s: string) => new TextEncoder().encode(s);

function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}

function u32le(value: number): Uint8Array {
  const out = new Uint8Array(4);
  new DataView(out.buffer).setUint32(0, value, true);
  return out;
}

function u64le(value: bigint): Uint8Array {
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, value, true);
  return out;
}

/** Solana's compact-u16 ("shortvec") length prefix. */
function shortvec(value: number): Uint8Array {
  const out: number[] = [];
  let rem = value;
  for (;;) {
    let byte = rem & 0x7f;
    rem >>= 7;
    if (rem === 0) {
      out.push(byte);
      return Uint8Array.from(out);
    }
    byte |= 0x80;
    out.push(byte);
  }
}

function isOnCurve(bytes: Uint8Array): boolean {
  try {
    // zip215 = true matches web3.js / the Solana runtime's permissive check.
    ed25519.Point.fromBytes(bytes, true);
    return true;
  } catch {
    return false;
  }
}

/** `PublicKey.findProgramAddressSync` equivalent. Returns base58. */
export function findProgramAddress(
  seeds: Uint8Array[],
  programId: string,
): string {
  const program = base58.decode(programId);
  for (let bump = 255; bump >= 0; bump--) {
    const hash = sha256(
      concat([...seeds, Uint8Array.of(bump), program, PDA_MARKER]),
    );
    if (!isOnCurve(hash)) return base58.encode(hash);
  }
  throw new Error("Unable to find a viable program address bump seed");
}

/** The owner's associated token account for `mint`. Pure, no RPC. */
export function deriveAssociatedTokenAccount(
  ownerBase58: string,
  mintBase58: string = SOLANA_USDC_MINT,
): string {
  return findProgramAddress(
    [
      base58.decode(ownerBase58),
      base58.decode(TOKEN_PROGRAM_ID),
      base58.decode(mintBase58),
    ],
    ASSOCIATED_TOKEN_PROGRAM_ID,
  );
}

/** Left-pad a 20-byte EVM address into a 32-byte CCTP recipient. */
export function evmAddressToBytes32(address: string): Uint8Array {
  const hex = address.toLowerCase().replace(/^0x/, "");
  if (!/^[0-9a-f]{40}$/.test(hex)) {
    throw new Error(`Not an EVM address: ${address}`);
  }
  const out = new Uint8Array(32);
  for (let i = 0; i < 20; i++) {
    out[12 + i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

// ============================================================================
// deposit_for_burn instruction
// ============================================================================

/** One account of an instruction, as Solana's `AccountMeta`. */
export interface SolanaAccountMeta {
  pubkey: string;
  isSigner: boolean;
  isWritable: boolean;
}

/** A Solana instruction with base58 account keys. */
export interface SolanaInstruction {
  programId: string;
  keys: SolanaAccountMeta[];
  data: Uint8Array;
}

export interface SolanaDepositForBurnParams {
  /** Wallet that owns the USDC and pays fees + the event account's rent. */
  owner: string;
  /** The fresh account the program writes the outgoing message into. */
  messageSentEventData: string;
  /** USDC amount in smallest units (6 decimals). */
  amount: bigint;
  /** CCTP destination domain (3 = Arbitrum). */
  destinationDomain: number;
  /** EVM address minted to on the destination (the Kernel account). */
  mintRecipient: string;
  /** EVM address allowed to call `receiveMessage` (the Kernel account). */
  destinationCaller: string;
  /** Max fast-transfer fee in USDC units. */
  maxFee: bigint;
  /** `FINALITY_FAST` (1000) or `FINALITY_STANDARD` (2000). */
  minFinalityThreshold?: number;
  /** USDC mint. Defaults to mainnet USDC. */
  usdcMint?: string;
}

/** The PDAs `deposit_for_burn` reads, derived from the CCTP program ids. */
export function depositForBurnPdas(
  owner: string,
  destinationDomain: number,
  usdcMint: string = SOLANA_USDC_MINT,
) {
  const tmm = SOLANA_TOKEN_MESSENGER_MINTER_V2;
  return {
    senderAuthority: findProgramAddress([utf8("sender_authority")], tmm),
    denylistAccount: findProgramAddress(
      [utf8("denylist_account"), base58.decode(owner)],
      tmm,
    ),
    messageTransmitter: findProgramAddress(
      [utf8("message_transmitter")],
      SOLANA_MESSAGE_TRANSMITTER_V2,
    ),
    tokenMessenger: findProgramAddress([utf8("token_messenger")], tmm),
    // The domain seed is its decimal string, not its bytes (Circle's
    // `getDepositForBurnPdas` passes `destinationDomain.toString()`).
    remoteTokenMessenger: findProgramAddress(
      [utf8("remote_token_messenger"), utf8(String(destinationDomain))],
      tmm,
    ),
    tokenMinter: findProgramAddress([utf8("token_minter")], tmm),
    localToken: findProgramAddress(
      [utf8("local_token"), base58.decode(usdcMint)],
      tmm,
    ),
    // Anchor `#[event_cpi]`: the program's own event authority.
    eventAuthority: findProgramAddress([utf8("__event_authority")], tmm),
  };
}

/**
 * Borsh-encode `DepositForBurnParams` behind the Anchor discriminator:
 * amount u64, destination_domain u32, mint_recipient [32], destination_caller
 * [32], max_fee u64, min_finality_threshold u32 (all little-endian).
 */
export function encodeDepositForBurnData(
  params: Pick<
    SolanaDepositForBurnParams,
    | "amount"
    | "destinationDomain"
    | "mintRecipient"
    | "destinationCaller"
    | "maxFee"
    | "minFinalityThreshold"
  >,
): Uint8Array {
  return concat([
    DEPOSIT_FOR_BURN_DISCRIMINATOR,
    u64le(params.amount),
    u32le(params.destinationDomain),
    evmAddressToBytes32(params.mintRecipient),
    evmAddressToBytes32(params.destinationCaller),
    u64le(params.maxFee),
    u32le(params.minFinalityThreshold ?? FINALITY_FAST),
  ]);
}

/**
 * The `deposit_for_burn` instruction. Account order is the IDL's
 * `DepositForBurnContext`, followed by the two accounts `#[event_cpi]`
 * appends (event authority, program).
 */
export function buildDepositForBurnInstruction(
  params: SolanaDepositForBurnParams,
): SolanaInstruction {
  const usdcMint = params.usdcMint ?? SOLANA_USDC_MINT;
  const pdas = depositForBurnPdas(
    params.owner,
    params.destinationDomain,
    usdcMint,
  );
  const ro = (pubkey: string): SolanaAccountMeta => ({
    pubkey,
    isSigner: false,
    isWritable: false,
  });
  const rw = (pubkey: string): SolanaAccountMeta => ({
    pubkey,
    isSigner: false,
    isWritable: true,
  });
  return {
    programId: SOLANA_TOKEN_MESSENGER_MINTER_V2,
    keys: [
      { pubkey: params.owner, isSigner: true, isWritable: false }, // owner
      { pubkey: params.owner, isSigner: true, isWritable: true }, // event_rent_payer
      ro(pdas.senderAuthority),
      rw(deriveAssociatedTokenAccount(params.owner, usdcMint)), // burn_token_account
      ro(pdas.denylistAccount),
      rw(pdas.messageTransmitter),
      ro(pdas.tokenMessenger),
      ro(pdas.remoteTokenMessenger),
      ro(pdas.tokenMinter),
      rw(pdas.localToken),
      rw(usdcMint), // burn_token_mint
      { pubkey: params.messageSentEventData, isSigner: true, isWritable: true },
      ro(SOLANA_MESSAGE_TRANSMITTER_V2),
      ro(SOLANA_TOKEN_MESSENGER_MINTER_V2),
      ro(TOKEN_PROGRAM_ID),
      ro(SYSTEM_PROGRAM_ID),
      ro(pdas.eventAuthority),
      ro(SOLANA_TOKEN_MESSENGER_MINTER_V2), // program (event_cpi)
    ],
    data: encodeDepositForBurnData(params),
  };
}

/** `ComputeBudget` instructions: unit limit + price (micro-lamports / CU). */
export function computeBudgetInstructions(
  unitLimit: number,
  microLamportsPerUnit: bigint,
): SolanaInstruction[] {
  return [
    {
      programId: COMPUTE_BUDGET_PROGRAM_ID,
      keys: [],
      data: concat([Uint8Array.of(2), u32le(unitLimit)]),
    },
    {
      programId: COMPUTE_BUDGET_PROGRAM_ID,
      keys: [],
      data: concat([Uint8Array.of(3), u64le(microLamportsPerUnit)]),
    },
  ];
}

// ============================================================================
// Legacy transaction wire format
// ============================================================================

/**
 * Compile instructions into a legacy message: account keys ordered as
 * writable signers (fee payer first), read-only signers, writable
 * non-signers, read-only non-signers; flags merged per key.
 */
export function compileLegacyMessage(
  feePayer: string,
  instructions: SolanaInstruction[],
  recentBlockhash: string,
): { message: Uint8Array; accountKeys: string[]; numSigners: number } {
  const metas = new Map<string, { isSigner: boolean; isWritable: boolean }>();
  const touch = (pubkey: string, isSigner: boolean, isWritable: boolean) => {
    const prev = metas.get(pubkey);
    metas.set(pubkey, {
      isSigner: (prev?.isSigner ?? false) || isSigner,
      isWritable: (prev?.isWritable ?? false) || isWritable,
    });
  };
  touch(feePayer, true, true);
  for (const ix of instructions) {
    for (const k of ix.keys) touch(k.pubkey, k.isSigner, k.isWritable);
    touch(ix.programId, false, false);
  }

  const entries = [...metas.entries()];
  const group = (signer: boolean, writable: boolean) =>
    entries
      .filter(([, m]) => m.isSigner === signer && m.isWritable === writable)
      .map(([k]) => k);
  const writableSigners = group(true, true).filter((k) => k !== feePayer);
  const readonlySigners = group(true, false);
  const writableNonSigners = group(false, true);
  const readonlyNonSigners = group(false, false);
  const accountKeys = [
    feePayer,
    ...writableSigners,
    ...readonlySigners,
    ...writableNonSigners,
    ...readonlyNonSigners,
  ];
  const index = new Map(accountKeys.map((k, i) => [k, i]));
  const numSigners = 1 + writableSigners.length + readonlySigners.length;

  const compiled = instructions.map((ix) => {
    const accounts = ix.keys.map((k) => index.get(k.pubkey) as number);
    return concat([
      Uint8Array.of(index.get(ix.programId) as number),
      shortvec(accounts.length),
      Uint8Array.from(accounts),
      shortvec(ix.data.length),
      ix.data,
    ]);
  });

  const message = concat([
    Uint8Array.of(
      numSigners,
      readonlySigners.length,
      readonlyNonSigners.length,
    ),
    shortvec(accountKeys.length),
    ...accountKeys.map((k) => base58.decode(k)),
    base58.decode(recentBlockhash),
    shortvec(compiled.length),
    ...compiled,
  ]);
  return { message, accountKeys, numSigners };
}

export interface SolanaBurnTransaction {
  /** Wire-format transaction; the fee payer's signature slot is empty. */
  transaction: Uint8Array;
  /** The compiled message (what every signer signs). */
  message: Uint8Array;
  /** The generated `message_sent_event_data` account (base58). */
  messageSentEventData: string;
}

export interface BuildSolanaBurnTransactionParams
  extends Omit<SolanaDepositForBurnParams, "messageSentEventData"> {
  recentBlockhash: string;
  /** Priority fee in micro-lamports per compute unit. */
  priorityFeeMicroLamports: bigint;
  /** Secret key of the event-data account. Generated when omitted (tests pin it). */
  eventDataSecretKey?: Uint8Array;
}

/**
 * Build the burn transaction, signed by the event-data keypair only. The
 * wallet adds the owner's signature (slot 0) when it signs and sends.
 */
export function buildSolanaBurnTransaction(
  params: BuildSolanaBurnTransactionParams,
): SolanaBurnTransaction {
  const secret = params.eventDataSecretKey ?? ed25519.utils.randomSecretKey();
  const eventPubkey = base58.encode(ed25519.getPublicKey(secret));
  const burn = buildDepositForBurnInstruction({
    ...params,
    messageSentEventData: eventPubkey,
  });
  const { message, accountKeys, numSigners } = compileLegacyMessage(
    params.owner,
    [
      ...computeBudgetInstructions(
        SOLANA_BURN_COMPUTE_UNIT_LIMIT,
        params.priorityFeeMicroLamports,
      ),
      burn,
    ],
    params.recentBlockhash,
  );

  const signatures: Uint8Array[] = accountKeys
    .slice(0, numSigners)
    .map((key) =>
      key === eventPubkey ? ed25519.sign(message, secret) : new Uint8Array(64),
    );
  return {
    transaction: concat([shortvec(numSigners), ...signatures, message]),
    message,
    messageSentEventData: eventPubkey,
  };
}
