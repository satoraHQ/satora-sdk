import {
  type ArkProvider,
  type ContractRepository,
  type IndexerProvider,
  type IWallet,
  type Network,
  Ramps,
  type WalletRepository,
} from "@arkade-os/sdk";
import {
  type EscrowFundedEvent,
  EscrowMonitor,
  type EscrowScriptOptions,
  EscrowVtxoScript,
} from "@satora/escrow";
import type { Client } from "@satora/swap";
import { hex } from "@scure/base";

/**
 * The swap-client surface EscrowClient depends on — just the methods it calls,
 * picked from `@satora/swap`'s `Client`.
 *
 * Narrowing to a structural subset (rather than the whole `Client` class) keeps
 * the dependency minimal and lets any compatible client be injected, even one
 * resolved from a different physical copy of the swap SDK (the `Client` class
 * has private fields, so the full class would be a nominal mismatch across
 * copies).
 */
export type SwapClient = Pick<
  Client,
  "createLightningToArkadeSwap" | "claimArkade" | "createArkadeToLightningSwap"
>;

/**
 * Dependencies for {@link EscrowClient}.
 *
 * The swap client is **injected** — escrow-client never imports the swap SDK
 * at runtime (only its type), so it carries none of that bundle's weight.
 * Consumers (e.g. Peach) already run a swap client; this reuses it.
 */
export interface EscrowClientConfig {
  /** The consumer's swap client (see {@link SwapClient}). */
  swap: SwapClient;
  /** ASP provider (for fees + release submission). */
  arkProvider: ArkProvider;
  /** Indexer provider used to watch escrow addresses. */
  indexerProvider: IndexerProvider;
  /** Contract repository for the escrow monitor. */
  contractRepository: ContractRepository;
  /** Wallet repository (vtxo storage) for the escrow monitor. */
  walletRepository: WalletRepository;
}

export interface FundFromLightningParams {
  /** The escrow to fund (script params, used to derive the address + watch). */
  escrow: EscrowScriptOptions;
  /** Network for address derivation. */
  network: Network;
  /** Sats to receive at the escrow (the LN→Arkade swap target amount). */
  amountSats: number;
}

export interface FundFromLightningHandle {
  /** Lendaswap swap id. */
  swapId: string;
  /** BOLT11 invoice the funder pays to start the LN→escrow swap. */
  invoice: string;
  /** The escrow Ark address being funded. */
  escrowAddress: string;
  /**
   * Call after the invoice has been paid. Claims the server-funded VHTLC into
   * the escrow address and resolves once the escrow VTXO is observed by the
   * monitor. Rejects on timeout.
   */
  awaitFunded(timeoutMs?: number): Promise<EscrowFundedEvent>;
}

/**
 * High-level escrow flows that bundle the swap on/off-ramp with the escrow
 * monitor: fund an escrow from Lightning, and withdraw a released payout to
 * Lightning or L1.
 *
 * The swap client is injected (see {@link EscrowClientConfig}). All escrow
 * primitives are re-exported from the package root, so a consumer needs only
 * `@satora/escrow-client` plus the swap `Client` it already runs.
 */
export class EscrowClient {
  private constructor(
    private readonly swap: SwapClient,
    private readonly arkProvider: ArkProvider,
    private readonly monitor: EscrowMonitor,
  ) {}

  static async create(config: EscrowClientConfig): Promise<EscrowClient> {
    const monitor = await EscrowMonitor.create({
      indexerProvider: config.indexerProvider,
      contractRepository: config.contractRepository,
      walletRepository: config.walletRepository,
    });
    return new EscrowClient(config.swap, config.arkProvider, monitor);
  }

  /** The underlying escrow monitor (onFunded/onReleased, watch, listEscrows). */
  get escrowMonitor(): EscrowMonitor {
    return this.monitor;
  }

  /**
   * Fund an escrow from Lightning.
   *
   * Creates a Lightning→Arkade swap whose payout claims into the escrow
   * address, and starts watching the escrow. Returns the invoice to pay plus
   * `awaitFunded()`, which — after the invoice is paid — claims the
   * server-funded VHTLC into the escrow and resolves when the VTXO lands.
   */
  async fundFromLightning(
    params: FundFromLightningParams,
  ): Promise<FundFromLightningHandle> {
    const script = new EscrowVtxoScript(params.escrow);
    const escrowAddress = script.arkAddress(params.network);
    const escrowPkScript = hex.encode(script.pkScript);

    // Watch the escrow so onFunded fires when the claim lands.
    await this.monitor.watch(params.escrow, params.network);

    const { response } = await this.swap.createLightningToArkadeSwap({
      satsReceive: params.amountSats,
      targetAddress: escrowAddress,
    });

    const awaitFunded = (timeoutMs = 120_000): Promise<EscrowFundedEvent> => {
      const funded = new Promise<EscrowFundedEvent>((resolve) => {
        const unsubscribe = this.monitor.onFunded((event) => {
          if (event.contract.script === escrowPkScript) {
            unsubscribe();
            resolve(event);
          }
        });
      });

      return (async () => {
        // Claim the server-funded VHTLC into the escrow. waitForVtxoMs absorbs
        // the lag between LN payment, server funding, and indexing.
        const claim = await this.swap.claimArkade(response.id, {
          destinationAddress: escrowAddress,
          waitForVtxoMs: timeoutMs,
        });
        if (!claim.success) {
          throw new Error(`claimArkade failed: ${claim.message}`);
        }
        return withTimeout(funded, timeoutMs, "escrow funding");
      })();
    };

    return {
      swapId: response.id,
      invoice: response.bolt11_invoice,
      escrowAddress,
      awaitFunded,
    };
  }

  /**
   * Withdraw a released payout to Lightning via an Arkade→Lightning swap.
   *
   * Creates the swap for `lightningInvoice` and funds its VHTLC from the buyer
   * wallet's payout. The Lendaswap server then claims the VHTLC and pays the
   * invoice. Returns the swap id, the VHTLC funding txid, and the sats sent
   * (the invoice amount plus the swap fee — must be <= the available payout).
   */
  async withdrawToLightning(params: {
    /** Buyer wallet holding the released payout VTXO(s). */
    wallet: IWallet;
    /** BOLT11 invoice to be paid by the swap server. */
    lightningInvoice: string;
  }): Promise<{
    swapId: string;
    fundingTxid: string;
    sourceAmountSats: number;
  }> {
    const { response } = await this.swap.createArkadeToLightningSwap({
      lightningInvoice: params.lightningInvoice,
    });
    // source_amount is serialized as a string over the wire.
    const sourceAmountSats = Number(response.source_amount);
    const fundingTxid = await params.wallet.send({
      address: response.arkade_vhtlc_address,
      amount: sourceAmountSats,
    });
    return { swapId: response.id, fundingTxid, sourceAmountSats };
  }

  /**
   * Withdraw a released payout to L1 (a collaborative Arkade offboard).
   *
   * Redeems the buyer wallet's virtual outputs to an onchain address via a
   * settlement round (`Ramps.offboard`). Returns the settlement txid.
   *
   * The buyer holds the released payout as a normal VTXO at their wallet's
   * address, so this is a plain offboard — no escrow-specific signing.
   */
  async withdrawToL1(params: {
    /** Buyer wallet holding the released payout VTXO(s). */
    wallet: IWallet;
    /** Destination onchain (L1) address. */
    destinationAddress: string;
    /** Amount to offboard in sats; omit to offboard everything. */
    amountSats?: bigint;
  }): Promise<string> {
    const { fees } = await this.arkProvider.getInfo();
    return new Ramps(params.wallet).offboard(
      params.destinationAddress,
      fees,
      params.amountSats,
    );
  }

  /** Release the monitor's resources (stop watching, clear listeners). */
  dispose(): void {
    this.monitor.dispose();
  }
}

function withTimeout<T>(p: Promise<T>, ms: number, label: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`timeout: ${label}`)), ms);
  });
  return Promise.race([p, timeout]).finally(() => {
    if (timer) clearTimeout(timer);
  });
}
