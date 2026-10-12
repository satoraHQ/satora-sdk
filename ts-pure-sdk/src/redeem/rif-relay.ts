/**
 * Claim of a native lock (RBTC on Rootstock) through RIF Relay.
 *
 * The lock's claim address is the recipient's RIF smart wallet, fixed at
 * swap creation (`rifSmartWalletAddress`). A relayed request from that smart
 * wallet calls `HTLCNative.redeem`, which pays the smart wallet; the smart
 * wallet then pays the relayer its fee and forwards the rest to its owner,
 * the recipient. Only the owner can sign that request, and the recipient
 * needs no RBTC: the fee comes out of the claimed coin. The first claim
 * deploys the smart wallet in the same request (through the factory); later
 * claims are relayed from the deployed wallet.
 */

import { keccak256 } from "../evm/coordinator.js";
import type { EIP712TypedData, EvmSigner } from "../evm/wallet.js";
import type { GaslessSwapResponse } from "./gasless.js";
import type { ClaimGaslessResult } from "./types.js";

/** A RIF Relay deployment, as `/chain-config` publishes it. */
export interface RifRelayConfig {
  /** Relay server base URL (`/chain-info`, `/estimate`, `/relay`). */
  server_url: string;
  relay_hub: string;
  smart_wallet_factory: string;
  /** The implementation the factory's smart wallet proxies delegate to. */
  smart_wallet_template: string;
  /** Verifier of a request that deploys the smart wallet. */
  deploy_verifier: string;
  /** Verifier of a request from a deployed smart wallet. */
  relay_verifier: string;
}

const ZERO_ADDRESS = "0x0000000000000000000000000000000000000000";

/**
 * The smart wallet index: one wallet per owner, deployed on its first claim
 * and reused afterwards.
 */
export const RIF_SMART_WALLET_INDEX = 0;

/**
 * Gas the smart wallet forwards to `HTLCNative.redeem` on a relay request
 * (~60k measured). The relay server rejects a request whose call needs more;
 * unused gas is not charged.
 */
export const RIF_CLAIM_CALL_GAS = 100_000n;

/**
 * Gas for the fee payment to the relayer (a plain transfer to its EOA), as
 * the reference client sets it: the transfer's estimate minus its 10.5k
 * base the wallet's own call covers.
 */
const RIF_FEE_TRANSFER_GAS = 10_500n;

/**
 * Headroom on the relay server's estimate, in percent: the server
 * re-estimates the signed request and rejects a fee below its figure.
 */
const RIF_FEE_HEADROOM_PERCENT = 110n;

/** How long a signed request stays valid. The server wants at least 12 h. */
const RIF_REQUEST_VALIDITY_SECS = 2 * 24 * 60 * 60;

/**
 * Upper bound the server's worker nonce must not exceed when it relays.
 * It only stops a relayer from sitting on the request; the HTLC timelock
 * and `validUntilTime` bound that already, and the SDK has no node to read
 * the worker's nonce from, so the bound is left open.
 */
const RIF_RELAY_MAX_NONCE = Number.MAX_SAFE_INTEGER;

const RELAY_DATA_TYPE = [
  { name: "gasPrice", type: "uint256" },
  { name: "feesReceiver", type: "address" },
  { name: "callForwarder", type: "address" },
  { name: "callVerifier", type: "address" },
];

/** `RelayRequest` of a deploy (factory domain): `recoverer` + `index`, no `gas`. */
const DEPLOY_REQUEST_TYPE = [
  { name: "relayHub", type: "address" },
  { name: "from", type: "address" },
  { name: "to", type: "address" },
  { name: "tokenContract", type: "address" },
  { name: "recoverer", type: "address" },
  { name: "value", type: "uint256" },
  { name: "nonce", type: "uint256" },
  { name: "tokenAmount", type: "uint256" },
  { name: "tokenGas", type: "uint256" },
  { name: "validUntilTime", type: "uint256" },
  { name: "index", type: "uint256" },
  { name: "data", type: "bytes" },
  { name: "relayData", type: "RelayData" },
];

/** `RelayRequest` from a deployed wallet (wallet domain): `gas`, no `recoverer` / `index`. */
const RELAY_REQUEST_TYPE = [
  { name: "relayHub", type: "address" },
  { name: "from", type: "address" },
  { name: "to", type: "address" },
  { name: "tokenContract", type: "address" },
  { name: "value", type: "uint256" },
  { name: "gas", type: "uint256" },
  { name: "nonce", type: "uint256" },
  { name: "tokenAmount", type: "uint256" },
  { name: "tokenGas", type: "uint256" },
  { name: "validUntilTime", type: "uint256" },
  { name: "data", type: "bytes" },
  { name: "relayData", type: "RelayData" },
];

/** What the relay server reports at `/chain-info`. */
interface RifChainInfo {
  feesReceiver: string;
  relayHubAddress: string;
  minGasPrice: string;
  chainId: string;
  ready: boolean;
}

/** What the relay server answers at `/estimate` (all decimal strings). */
interface RifEstimate {
  gasPrice: string;
  estimation: string;
  requiredNativeAmount: string;
}

/** The smart wallet a claim is relayed from. */
export interface RifSmartWallet {
  address: string;
  /** Whether it exists on chain; if not, the claim deploys it. */
  deployed: boolean;
  /** The nonce the next request must carry (factory's per owner, or the wallet's). */
  nonce: bigint;
}

/** One request, before the fee is fixed. */
interface UnpricedRequest {
  wallet: RifSmartWallet;
  owner: string;
  data: string;
  htlc: string;
  relay: RifRelayConfig;
  chainInfo: RifChainInfo;
  validUntilTime: number;
}

export interface RifRelayClaimParams {
  swap: GaslessSwapResponse;
  /** The swap preimage (hex, with or without 0x prefix) */
  preimage: string;
  /** Owner of the smart wallet: signs the relay request, receives the coin. */
  signer: EvmSigner;
  relay: RifRelayConfig;
  /** Test seam; defaults to the global `fetch`. */
  fetch?: typeof fetch;
  /** Test seam: seconds since the epoch. */
  now?: () => number;
}

/**
 * Claim a native lock through RIF Relay. The coin, less the relayer's fee,
 * lands at `signer.address`, which must own the lock's claim address.
 */
export async function claimNativeViaRifRelay(
  params: RifRelayClaimParams,
): Promise<ClaimGaslessResult> {
  const { swap, preimage, signer, relay } = params;
  const fetchFn = params.fetch ?? fetch;
  const now = params.now ?? (() => Math.floor(Date.now() / 1000));
  if (signer.chainId !== swap.evm_chain_id) {
    throw new Error(
      `Wallet is on chain ${signer.chainId}, but the claim must be sent on chain ${swap.evm_chain_id}.`,
    );
  }

  const chainInfo = await rifGet<RifChainInfo>(
    fetchFn,
    relay.server_url,
    "/chain-info",
  );
  if (!chainInfo.ready) {
    throw new Error("RIF Relay server is not ready; retry in a minute.");
  }
  if (Number(chainInfo.chainId) !== swap.evm_chain_id) {
    throw new Error(
      `RIF Relay server serves chain ${chainInfo.chainId}, the swap is on chain ${swap.evm_chain_id}.`,
    );
  }

  // Only the owner of the lock's claim address can claim it.
  const owner = signer.address;
  const claimAddress = swap.client_evm_address;
  if (
    rifSmartWalletAddress(relay, owner).toLowerCase() !==
    claimAddress.toLowerCase()
  ) {
    throw new Error(
      `Wallet ${owner} does not own this swap's claim address ${claimAddress}; ` +
        "connect the wallet the swap pays out to.",
    );
  }
  const wallet = await rifSmartWallet(signer, relay, owner);

  // The redeem pays the smart wallet (the lock's claim address): that is
  // what the verifier credits toward the fee, and the wallet forwards the rest.
  const unpriced: UnpricedRequest = {
    wallet,
    owner,
    data: encodeNativeRedeem({
      preimage: preimage.startsWith("0x") ? preimage : `0x${preimage}`,
      amount: BigInt(swap.evm_expected_sats),
      sender: swap.server_evm_address,
      timelock: BigInt(swap.evm_refund_locktime),
    }),
    htlc: swap.evm_htlc_address,
    relay,
    chainInfo,
    validUntilTime: now() + RIF_REQUEST_VALIDITY_SECS,
  };

  // Estimate unsigned with no fee, as the server requires, at the floor
  // price; then price the fee at the higher of that and the server's price.
  const minGasPrice = BigInt(chainInfo.minGasPrice);
  const estimate = await rifPost<RifEstimate>(
    fetchFn,
    relay.server_url,
    "/estimate",
    rifEnvelope(
      buildRifRequest(unpriced, 0n, minGasPrice),
      "SERVER_SIGNATURE_REQUIRED",
    ),
  );
  const serverGasPrice = BigInt(estimate.gasPrice);
  const gasPrice = serverGasPrice > minGasPrice ? serverGasPrice : minGasPrice;
  const fee = rifFee(BigInt(estimate.estimation), gasPrice);
  const amount = BigInt(swap.evm_expected_sats);
  if (fee >= amount) {
    throw new Error(
      `RIF Relay fee ${fee} wei is not below the claimed ${amount} wei.`,
    );
  }

  const request = buildRifRequest(unpriced, fee, gasPrice);
  const signature = await signer.signTypedData(
    rifTypedData(request, swap.evm_chain_id),
  );
  const relayed = await rifPost<{ signedTx: string; txHash?: string }>(
    fetchFn,
    relay.server_url,
    "/relay",
    rifEnvelope(request, signature),
  );
  const txHash = relayed.txHash ?? keccak256(relayed.signedTx);

  const receipt = await signer.waitForReceipt(txHash);
  if (receipt.status !== "success") {
    throw new Error(`RIF Relay claim transaction ${txHash} reverted.`);
  }
  return {
    id: swap.id,
    status: "clientredeemed",
    txHash,
    message: `HTLCNative.redeem relayed by RIF Relay from smart wallet ${wallet.address} (fee ${fee} wei)`,
  };
}

/** `estimation × gasPrice` plus headroom. */
export function rifFee(estimation: bigint, gasPrice: bigint): bigint {
  return (estimation * gasPrice * RIF_FEE_HEADROOM_PERCENT) / 100n;
}

/** A RIF request: the signed fields, as the server's JSON wants them. */
export interface RifRequest {
  kind: "deploy" | "relay";
  /** EIP-712 `verifyingContract`: the factory for a deploy, else the wallet. */
  verifyingContract: string;
  request: Record<string, string | number>;
  relayData: {
    gasPrice: string;
    feesReceiver: string;
    callForwarder: string;
    callVerifier: string;
  };
  relayHub: string;
}

function buildRifRequest(
  u: UnpricedRequest,
  tokenAmount: bigint,
  gasPrice: bigint,
): RifRequest {
  const tokenGas = tokenAmount === 0n ? 0n : RIF_FEE_TRANSFER_GAS;
  const common = {
    relayHub: u.relay.relay_hub,
    from: u.owner,
    to: u.htlc,
    tokenContract: ZERO_ADDRESS,
    value: "0",
    nonce: u.wallet.nonce.toString(),
    tokenAmount: tokenAmount.toString(),
    tokenGas: tokenGas.toString(),
    validUntilTime: u.validUntilTime,
    data: u.data,
  };
  const feesReceiver = u.chainInfo.feesReceiver;
  if (!u.wallet.deployed) {
    return {
      kind: "deploy",
      verifyingContract: u.relay.smart_wallet_factory,
      request: {
        ...common,
        recoverer: ZERO_ADDRESS,
        index: RIF_SMART_WALLET_INDEX,
      },
      relayData: {
        gasPrice: gasPrice.toString(),
        feesReceiver,
        callForwarder: u.relay.smart_wallet_factory,
        callVerifier: u.relay.deploy_verifier,
      },
      relayHub: u.relay.relay_hub,
    };
  }
  return {
    kind: "relay",
    verifyingContract: u.wallet.address,
    request: { ...common, gas: RIF_CLAIM_CALL_GAS.toString() },
    relayData: {
      gasPrice: gasPrice.toString(),
      feesReceiver,
      callForwarder: u.wallet.address,
      callVerifier: u.relay.relay_verifier,
    },
    relayHub: u.relay.relay_hub,
  };
}

/** The `RelayRequest` the owner signs, for `EvmSigner.signTypedData`. */
export function rifTypedData(
  req: RifRequest,
  chainId: number,
): EIP712TypedData {
  const uint = new Set([
    "value",
    "gas",
    "nonce",
    "tokenAmount",
    "tokenGas",
    "validUntilTime",
    "index",
  ]);
  const message: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(req.request)) {
    message[key] = uint.has(key) ? BigInt(value) : value;
  }
  message.relayData = {
    ...req.relayData,
    gasPrice: BigInt(req.relayData.gasPrice),
  };
  return {
    domain: {
      name: "RSK Enveloping Transaction",
      version: "2",
      chainId,
      verifyingContract: req.verifyingContract,
    },
    types: {
      RelayRequest:
        req.kind === "deploy" ? DEPLOY_REQUEST_TYPE : RELAY_REQUEST_TYPE,
      RelayData: RELAY_DATA_TYPE,
    },
    primaryType: "RelayRequest",
    message,
  };
}

function rifEnvelope(req: RifRequest, signature: string) {
  return {
    relayRequest: { request: req.request, relayData: req.relayData },
    metadata: {
      relayHubAddress: req.relayHub,
      relayMaxNonce: RIF_RELAY_MAX_NONCE,
      signature,
    },
  };
}

// keccak256("redeem(bytes32,uint256,address,uint256)")[:4]
const HTLC_NATIVE_REDEEM_SELECTOR = "0xd592e483";
// keccak256("nonce(address)")[:4], the factory's per-owner deploy nonce
const FACTORY_NONCE_SELECTOR = "0x70ae92d2";
// keccak256("nonce()")[:4], the wallet's relay nonce
const WALLET_NONCE_SELECTOR = "0xaffed0e0";

/** `HTLCNative.redeem(preimage, amount, sender, timelock)`, called by the claim address. */
function encodeNativeRedeem(p: {
  preimage: string;
  amount: bigint;
  sender: string;
  timelock: bigint;
}): string {
  return (
    HTLC_NATIVE_REDEEM_SELECTOR +
    word(p.preimage) +
    word(p.amount) +
    word(p.sender) +
    word(p.timelock)
  );
}

/**
 * The smart wallet `owner` claims from: the factory's CREATE2 proxy for
 * (`owner`, no recoverer, `RIF_SMART_WALLET_INDEX`) around the template, as
 * `factory.getSmartWalletAddress` computes it. A native lock on a RIF Relay
 * chain is created with this as its claim address. Lowercase hex.
 */
export function rifSmartWalletAddress(
  relay: RifRelayConfig,
  owner: string,
): string {
  const salt = keccak256(
    `0x${strip0x(owner).toLowerCase()}${"0".repeat(40)}${word(BigInt(RIF_SMART_WALLET_INDEX))}`,
  );
  const initCodeHash = keccak256(
    `0x602d3d8160093d39f3363d3d373d3d3d3d363d73${strip0x(relay.smart_wallet_template).toLowerCase()}5af43d923d90803e602b57fd5bf3`,
  );
  const hash = keccak256(
    `0xff${strip0x(relay.smart_wallet_factory).toLowerCase()}${strip0x(salt)}${strip0x(initCodeHash)}`,
  );
  return `0x${hash.slice(-40)}`;
}

/**
 * The owner's smart wallet, whether it is deployed, and the nonce its next
 * request carries.
 */
export async function rifSmartWallet(
  signer: EvmSigner,
  relay: RifRelayConfig,
  owner: string,
): Promise<RifSmartWallet> {
  const address = rifSmartWalletAddress(relay, owner);
  // A wallet that is not deployed has no code: the call returns no data.
  const walletNonce = strip0x(
    await signer.call({ to: address, data: WALLET_NONCE_SELECTOR }),
  );
  if (walletNonce.length >= 64) {
    return { address, deployed: true, nonce: BigInt(`0x${walletNonce}`) };
  }
  const factoryNonce = await signer.call({
    to: relay.smart_wallet_factory,
    data: FACTORY_NONCE_SELECTOR + word(owner),
  });
  return { address, deployed: false, nonce: BigInt(factoryNonce) };
}

function strip0x(hex: string): string {
  return hex.startsWith("0x") ? hex.slice(2) : hex;
}

function word(value: string | bigint): string {
  const hex =
    typeof value === "bigint"
      ? value.toString(16)
      : strip0x(value).toLowerCase();
  return hex.padStart(64, "0");
}

async function rifGet<T>(
  fetchFn: typeof fetch,
  baseUrl: string,
  path: string,
): Promise<T> {
  return rifResponse<T>(await fetchFn(`${baseUrl}${path}`), path);
}

async function rifPost<T>(
  fetchFn: typeof fetch,
  baseUrl: string,
  path: string,
  body: unknown,
): Promise<T> {
  const response = await fetchFn(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  return rifResponse<T>(response, path);
}

/** The server answers rejections with HTTP 200 and `{ error }`. */
async function rifResponse<T>(response: Response, path: string): Promise<T> {
  const text = await response.text();
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    throw new Error(
      `RIF Relay ${path} answered ${response.status}: ${text.slice(0, 200)}`,
    );
  }
  const error = (body as { error?: unknown }).error;
  if (!response.ok || error != null) {
    throw new Error(
      `RIF Relay ${path} rejected the request: ${typeof error === "string" ? error : text.slice(0, 200)}`,
    );
  }
  return body as T;
}

/**
 * The swap's claim goes through RIF Relay and needs the claim destination's
 * wallet to sign: `claim()` cannot do it unattended. Call
 * `claimEvmWithSigner(id, signer)` with that wallet.
 */
export class WalletClaimRequiredError extends Error {
  constructor(
    readonly swapId: string,
    readonly chainId: number,
  ) {
    super(
      `Swap ${swapId} is claimed through RIF Relay on chain ${chainId}: ` +
        "call claimEvmWithSigner with the wallet that receives the coin.",
    );
    this.name = "WalletClaimRequiredError";
  }
}
