/**
 * EVM wallet / signer abstraction.
 *
 * Consumers inject an implementation that wraps their wallet library
 * (wagmi, viem, ethers, etc.). The SDK uses this interface to sign
 * typed data and send transactions without depending on any specific
 * EVM library.
 */

// ── EIP-712 typed data ───────────────────────────────────────────────────────

/** Generic EIP-712 typed data that can be passed to a wallet for signing. */
export interface EIP712TypedData {
  domain: {
    name?: string;
    version?: string;
    chainId?: number;
    verifyingContract?: string;
  };
  types: Record<string, Array<{ name: string; type: string }>>;
  primaryType: string;
  message: Record<string, unknown>;
}

// ── Signer interface ─────────────────────────────────────────────────────────

/**
 * Minimal EVM signer that the SDK needs to fund and refund swaps.
 *
 * Example implementation using wagmi/viem:
 * ```ts
 * const signer: EvmSigner = {
 *   address: walletClient.account.address,
 *   chainId: walletClient.chain.id,
 *   signTypedData: (td) => walletClient.signTypedData({ ...td, account: walletClient.account }),
 *   sendTransaction: (tx) => walletClient.sendTransaction({ to: tx.to, data: tx.data, chain, gas: tx.gas }),
 *   waitForReceipt: (hash) => publicClient.waitForTransactionReceipt({ hash }),
 *   getTransaction: (hash) => publicClient.getTransaction({ hash }),
 *   call: (tx) => publicClient.call({ to: tx.to, data: tx.data, account: tx.from, blockNumber: tx.blockNumber }),
 * };
 * ```
 */
export interface EvmSigner {
  /** The connected wallet address (checksummed or lowercase hex). */
  address: string;
  /** Current chain ID the wallet is connected to. */
  chainId: number;

  /**
   * Sign EIP-712 typed data.
   * Must return the 65-byte hex signature (0x-prefixed).
   */
  signTypedData(typedData: EIP712TypedData): Promise<string>;

  /**
   * Sign a raw message hash (personal_sign style).
   *
   * **Required for the CCTP-inbound flow** — Kernel's ECDSA validator
   * signs the UserOp hash via `signMessage({ raw })` on the owner.
   * The direct-Permit2 path does not call this, so existing consumers
   * can leave it unimplemented.
   *
   * For wagmi/Privy/viem consumers this is a one-liner:
   * `(m) => walletClient.signMessage({ account, message: m })`.
   *
   * Must return the 65-byte hex signature (0x-prefixed).
   */
  signMessage?(message: { raw: string }): Promise<string>;

  /**
   * Send a raw transaction and return the transaction hash (0x-prefixed).
   *
   * @param tx.to - Target contract address (0x-prefixed)
   * @param tx.data - ABI-encoded calldata (0x-prefixed)
   * @param tx.gas - Optional gas limit; the SDK provides sensible defaults
   */
  sendTransaction(tx: {
    to: string;
    data: string;
    gas?: bigint;
  }): Promise<string>;

  /**
   * Wait for a transaction to be mined and return the receipt.
   *
   * The implementation should handle transaction replacements (speed-up /
   * cancel) — e.g. viem's `waitForTransactionReceipt` and ethers'
   * `provider.waitForTransaction` both do this automatically.
   *
   * @param hash - Transaction hash to wait for (0x-prefixed)
   */
  waitForReceipt(hash: string): Promise<TxReceipt>;

  /**
   * Get a transaction by hash. Used internally to replay reverted
   * transactions and extract on-chain revert reasons.
   *
   * @param hash - Transaction hash (0x-prefixed)
   */
  getTransaction(hash: string): Promise<{
    /** Target address, or null for contract creation */
    to: string | null;
    /** ABI-encoded calldata (0x-prefixed) */
    input: string;
    /** Sender address */
    from: string;
  }>;

  /**
   * Simulate a call via `eth_call`. Used internally to dry-run
   * transactions before sending and to extract revert reasons.
   *
   * @param tx.to - Target contract address (0x-prefixed)
   * @param tx.data - ABI-encoded calldata (0x-prefixed)
   * @param tx.from - Optional sender address for simulation context
   * @param tx.blockNumber - Optional block number to simulate against
   */
  call(tx: {
    to: string;
    data: string;
    from?: string;
    blockNumber?: bigint;
  }): Promise<string>;
}

// ── Transaction receipt ──────────────────────────────────────────────────────

export interface TxReceipt {
  status: "success" | "reverted";
  blockNumber: bigint;
  transactionHash: string;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/**
 * Normalise a 20-byte address to a 32-byte ABI-encoded word (no 0x prefix).
 */
function padAddress(addr: string): string {
  return addr.replace(/^0x/i, "").toLowerCase().padStart(64, "0");
}

/**
 * Encode a uint256 as a 32-byte ABI word (no 0x prefix).
 */
function encodeUint256(value: bigint): string {
  return value.toString(16).padStart(64, "0");
}

// ERC-20 function selectors
const ALLOWANCE_SELECTOR = "0xdd62ed3e"; // allowance(address,address)
const BALANCE_SELECTOR = "0x70a08231"; // balanceOf(address)
const APPROVE_SELECTOR = "0x095ea7b3"; // approve(address,uint256)
const TRANSFER_SELECTOR = "0xa9059cbb"; // transfer(address,uint256)

const MAX_UINT256 = BigInt(
  "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
);

/**
 * Encode an `allowance(owner, spender)` call.
 */
export function encodeAllowanceCall(
  tokenAddress: string,
  owner: string,
  spender: string,
): { to: string; data: string } {
  return {
    to: tokenAddress,
    data: `${ALLOWANCE_SELECTOR}${padAddress(owner)}${padAddress(spender)}`,
  };
}

/**
 * Encode a `balanceOf(account)` call.
 */
export function encodeBalanceOfCall(
  tokenAddress: string,
  account: string,
): { to: string; data: string } {
  return {
    to: tokenAddress,
    data: `${BALANCE_SELECTOR}${padAddress(account)}`,
  };
}

/**
 * Encode an `approve(spender, type(uint256).max)` transaction.
 */
export function encodeMaxApproveData(
  tokenAddress: string,
  spender: string,
): { to: string; data: string } {
  return {
    to: tokenAddress,
    data: `${APPROVE_SELECTOR}${padAddress(spender)}${encodeUint256(MAX_UINT256)}`,
  };
}

/**
 * Encode a `transfer(to, amount)` transaction.
 */
export function encodeTransferCall(
  tokenAddress: string,
  to: string,
  amount: bigint,
): { to: string; data: string } {
  return {
    to: tokenAddress,
    data: `${TRANSFER_SELECTOR}${padAddress(to)}${encodeUint256(amount)}`,
  };
}

/**
 * Decode a 32-byte ABI-encoded uint256 from a hex string.
 */
export function decodeUint256(hex: string): bigint {
  const clean = hex.replace(/^0x/i, "");
  if (clean.length === 0) return 0n;
  return BigInt(`0x${clean}`);
}

// ── Simulation ───────────────────────────────────────────────────────────────

/**
 * Simulate a transaction via `eth_call`. Throws with the revert reason
 * if the call would fail, so callers don't burn gas on doomed transactions.
 */
export async function simulateTransaction(
  signer: EvmSigner,
  tx: { to: string; data: string },
  label: string,
): Promise<void> {
  try {
    await signer.call({ to: tx.to, data: tx.data, from: signer.address });
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    const match =
      msg.match(/reverted with.*?:\s*(.+)/i) ?? msg.match(/reason:\s*(.+)/i);
    const reason = match?.[1]?.trim() ?? msg;
    throw new Error(`${label} would revert: ${reason}`);
  }
}

// ── Revert reason extraction ─────────────────────────────────────────────────

/**
 * Replay a reverted transaction to extract the on-chain revert reason.
 */
export async function getRevertReason(
  signer: EvmSigner,
  txHash: string,
  blockNumber: bigint,
): Promise<string> {
  try {
    const tx = await signer.getTransaction(txHash);
    if (!tx.to) return "Transaction reverted";
    await signer.call({
      to: tx.to,
      data: tx.input,
      from: tx.from,
      blockNumber,
    });
    return "Transaction reverted";
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    const match =
      msg.match(/reverted with.*?:\s*(.+)/i) ?? msg.match(/reason:\s*(.+)/i);
    return match?.[1]?.trim() ?? msg;
  }
}

// ── Error classification ─────────────────────────────────────────────────────

/**
 * Returns true if the error message indicates the user rejected the
 * transaction in their wallet (MetaMask, etc.).
 */
export function isUserRejection(error: unknown): boolean {
  const msg = error instanceof Error ? error.message : String(error);
  return /user rejected|user denied|rejected the request/i.test(msg);
}

// ── Signature parsing ────────────────────────────────────────────────────────

/**
 * Parse a 65-byte hex signature into its `v`, `r`, `s` components.
 */
export function parseSignature(signature: string): {
  v: number;
  r: string;
  s: string;
} {
  const hex = signature.replace(/^0x/, "");
  return {
    r: `0x${hex.slice(0, 64)}`,
    s: `0x${hex.slice(64, 128)}`,
    v: Number.parseInt(hex.slice(128, 130), 16),
  };
}
