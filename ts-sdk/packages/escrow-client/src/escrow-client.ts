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
import { classifyDestination, toLightningDestination } from "./destination.js";

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
  | "createLightningToArkadeSwap"
  | "claimArkade"
  | "createArkadeToLightningSwap"
  | "getArkadeToLightningQuote"
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

/**
 * Result of {@link EscrowClient.withdraw}, discriminated by `method`. `txid` is
 * present on every branch — the VHTLC funding txid (lightning), the offboard
 * settlement txid (l1), or the Ark transfer txid (arkade).
 */
export type WithdrawResult =
  | {
      method: "lightning";
      txid: string;
      swapId: string;
      sourceAmountSats: number;
    }
  | { method: "l1"; txid: string }
  | { method: "arkade"; txid: string };

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
   * Quote a Lightning withdrawal: given how many sats you'd spend from the
   * payout (`sourceAmountSats`), returns what the recipient receives after the
   * swap fee and what is actually spent.
   *
   * Use `recipientSats` as the `amountSats` for an LNURL / Lightning-address
   * {@link withdrawToLightning} — e.g. pre-fill the amount from the available
   * payout balance so the user doesn't have to compute fees by hand.
   */
  async quoteLightningWithdrawal(sourceAmountSats: number): Promise<{
    /** Sats the recipient receives (pass as `amountSats` to withdrawToLightning). */
    recipientSats: number;
    /** Sats actually spent from the payout to fund the swap VHTLC. */
    sourceSats: number;
  }> {
    const quote = await this.swap.getArkadeToLightningQuote(sourceAmountSats);
    // Amounts are serialized as strings over the wire.
    return {
      recipientSats: Number(quote.net_target_amount),
      sourceSats: Number(quote.net_source_amount),
    };
  }

  /**
   * Withdraw a released payout to Lightning via an Arkade→Lightning swap.
   *
   * `destination` may be a BOLT11 invoice, an LNURL (`lnurl1...`), or a
   * Lightning address (`user@host`) — the swap backend resolves LNURL /
   * address itself, so nothing is fetched client-side. `amountSats` (what the
   * recipient receives) is **required** for LNURL / address and **ignored**
   * for a BOLT11 invoice (the amount is taken from the invoice).
   *
   * Creates the swap and funds its VHTLC from the buyer wallet's payout; the
   * Lendaswap server then claims the VHTLC and pays the recipient. Returns the
   * swap id, the VHTLC funding txid, and the sats sent (recipient amount plus
   * the swap fee — must be <= the available payout).
   */
  async withdrawToLightning(params: {
    /** Buyer wallet holding the released payout VTXO(s). */
    wallet: IWallet;
    /** BOLT11 invoice, LNURL (`lnurl1...`), or Lightning address (`user@host`). */
    destination: string;
    /** Recipient amount in sats. Required for LNURL / address; ignored for BOLT11. */
    amountSats?: number;
  }): Promise<{
    swapId: string;
    fundingTxid: string;
    sourceAmountSats: number;
  }> {
    const { response } = await this.swap.createArkadeToLightningSwap(
      toLightningDestination(params.destination, params.amountSats),
    );
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

  /**
   * Withdraw a released payout to another Arkade address — a plain offchain Ark
   * transfer. The funds stay on Ark (no swap, no settlement round), so this is
   * the cheapest, fastest withdrawal. Returns the ark txid.
   *
   * The buyer holds the released payout as a normal VTXO at their wallet's
   * address, so this is a plain send — no escrow-specific signing.
   */
  async withdrawToArkade(params: {
    /** Buyer wallet holding the released payout VTXO(s). */
    wallet: IWallet;
    /** Destination Arkade address. */
    destinationAddress: string;
    /** Amount to send in sats. */
    amountSats: number;
  }): Promise<string> {
    return params.wallet.send({
      address: params.destinationAddress,
      amount: params.amountSats,
    });
  }

  /**
   * Smart withdrawal: route the payout to wherever `destination` points by
   * inspecting the string, dispatching to {@link withdrawToLightning},
   * {@link withdrawToArkade}, or {@link withdrawToL1}.
   *
   * - BOLT11 invoice / LNURL / Lightning address → Lightning
   * - Arkade address (`ark1…` / `tark1…`)        → offchain Ark transfer
   * - anything else (a Bitcoin address)          → L1 offboard
   *
   * `amountSats` is **required** for an Arkade address and for a Lightning
   * LNURL/address; **optional** for L1 (omit to offboard everything) and
   * **ignored** for a BOLT11 invoice. The result is discriminated by `method`;
   * `txid` is present for every branch.
   */
  async withdraw(params: {
    wallet: IWallet;
    /** Lightning (invoice/LNURL/address), Arkade, or onchain Bitcoin address. */
    destination: string;
    amountSats?: number;
  }): Promise<WithdrawResult> {
    const destination = params.destination.trim();

    switch (classifyDestination(destination)) {
      case "lightning": {
        const { swapId, fundingTxid, sourceAmountSats } =
          await this.withdrawToLightning({
            wallet: params.wallet,
            destination,
            amountSats: params.amountSats,
          });
        return {
          method: "lightning",
          txid: fundingTxid,
          swapId,
          sourceAmountSats,
        };
      }
      case "arkade": {
        if (params.amountSats === undefined) {
          throw new Error(
            "amountSats is required to withdraw to an Arkade address",
          );
        }
        const txid = await this.withdrawToArkade({
          wallet: params.wallet,
          destinationAddress: destination,
          amountSats: params.amountSats,
        });
        return { method: "arkade", txid };
      }
      case "l1": {
        const txid = await this.withdrawToL1({
          wallet: params.wallet,
          destinationAddress: destination,
          amountSats:
            params.amountSats === undefined
              ? undefined
              : BigInt(params.amountSats),
        });
        return { method: "l1", txid };
      }
    }
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
