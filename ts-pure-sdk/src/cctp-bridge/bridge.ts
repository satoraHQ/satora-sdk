import {
  BridgeChain,
  type BridgeChainIdentifier,
  type BridgeConfig,
  BridgeKit,
  type BridgeParams,
  type BridgeResult,
  type EstimateResult,
} from "@circle-fin/bridge-kit";

type BridgeKitAdapter = BridgeParams["from"]["adapter"];

// biome-ignore lint/suspicious/noExplicitAny: bridge-kit event payloads are discriminated by name; a concrete type here would require importing every action type.
type BridgeEventPayload = any;

export interface BridgeUsdcParams {
  /** Source chain + pre-built bridge-kit adapter. */
  source: {
    adapter: BridgeKitAdapter;
    chain: BridgeChainIdentifier;
    /** Required when the adapter is developer-controlled. */
    address?: string;
  };
  /**
   * Destination. Adapter may be the same instance as `source.adapter` if the
   * user holds the same key on both chains (the common case).
   */
  destination: {
    adapter: BridgeKitAdapter;
    chain: BridgeChainIdentifier;
    /** Required when the adapter is developer-controlled. */
    address?: string;
    /** Send to a different EVM address than the adapter's own. */
    recipientAddress?: string;
  };
  /** Human-readable USDC amount, e.g. "10.50". */
  amount: string;
  config?: BridgeConfig;
  /** Subscribe to all bridge-kit lifecycle events (approve/burn/attest/mint). */
  onEvent?: (event: BridgeEventPayload) => void;
  /** Subscribe to a single lifecycle event. */
  onApprove?: (event: BridgeEventPayload) => void;
  onBurn?: (event: BridgeEventPayload) => void;
  onAttestation?: (event: BridgeEventPayload) => void;
  onMint?: (event: BridgeEventPayload) => void;
}

/**
 * Bridge USDC cross-chain via Circle CCTPv2.
 *
 * Runs the full approve → burn → attest → mint flow in one call. Returns a
 * `BridgeResult` whose `state` is `'success'` or `'error'`; inspect `steps`
 * for per-action details. Recoverable failures can be resumed with
 * `BridgeKit.retry()` — see bridge-kit's retry guide.
 */
export async function bridgeUsdc(
  params: BridgeUsdcParams,
): Promise<BridgeResult> {
  const kit = new BridgeKit();
  attachListeners(kit, params);
  return kit.bridge(buildBridgeParams(params));
}

/**
 * Bridge USDC from any CCTPv2-supported chain into Arbitrum.
 *
 * Convenience for the common Lendaswap flow: land USDC on Arbitrum so the
 * existing Arbitrum USDC → BTC HTLC swap can run against it.
 */
export async function bridgeUsdcToArbitrum(
  params: Omit<BridgeUsdcParams, "destination"> & {
    destination: Omit<BridgeUsdcParams["destination"], "chain"> & {
      chain?: BridgeChainIdentifier;
    };
  },
): Promise<BridgeResult> {
  return bridgeUsdc({
    ...params,
    destination: {
      ...params.destination,
      chain: params.destination.chain ?? BridgeChain.Arbitrum,
    },
  });
}

/**
 * Estimate CCTPv2 bridge costs (protocol fee + gas on source and destination)
 * without executing anything. Requires an adapter on both sides because gas
 * estimation depends on the specific wallet/account.
 */
export async function estimateUsdcBridgeFees(
  params: Omit<
    BridgeUsdcParams,
    "onEvent" | "onApprove" | "onBurn" | "onAttestation" | "onMint"
  >,
): Promise<EstimateResult> {
  const kit = new BridgeKit();
  return kit.estimate(buildBridgeParams(params));
}

function buildBridgeParams(params: BridgeUsdcParams): BridgeParams {
  const from = {
    adapter: params.source.adapter,
    chain: params.source.chain,
    ...(params.source.address ? { address: params.source.address } : {}),
  } as BridgeParams["from"];

  const to = {
    adapter: params.destination.adapter,
    chain: params.destination.chain,
    ...(params.destination.address
      ? { address: params.destination.address }
      : {}),
    ...(params.destination.recipientAddress
      ? { recipientAddress: params.destination.recipientAddress }
      : {}),
  } as BridgeParams["to"];

  return {
    from,
    to,
    amount: params.amount,
    ...(params.config ? { config: params.config } : {}),
  };
}

function attachListeners(kit: BridgeKit, params: BridgeUsdcParams): void {
  if (params.onEvent) kit.on("*", params.onEvent);
  if (params.onApprove) kit.on("approve", params.onApprove);
  if (params.onBurn) kit.on("burn", params.onBurn);
  if (params.onAttestation) kit.on("fetchAttestation", params.onAttestation);
  if (params.onMint) kit.on("mint", params.onMint);
}
