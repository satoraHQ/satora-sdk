/** Native Spark BTC to EVM creation. Funds must be sent as an HTLC. */
import { deriveEvmAddress } from "../evm/signing.js";
import { bytesToHex } from "../signer/index.js";
import {
  assertAddressEchoed,
  assertSparkFunding,
} from "../swap-verification.js";
import { retryOnHashCollision } from "./retry.js";
import type {
  CreateSwapContext,
  SparkToEvmSwapOptions,
  SparkToEvmSwapResult,
} from "./types.js";

export async function createSparkToEvmSwap(
  options: SparkToEvmSwapOptions,
  ctx: CreateSwapContext,
  /**
   * The lock's claim address when it is not this swap's own key: the
   * recipient's RIF Relay smart wallet on a chain that claims through RIF.
   */
  claimingAddressOverride?: string,
): Promise<SparkToEvmSwapResult> {
  if ((options.sourceAmount == null) === (options.targetAmount == null)) {
    throw new Error("Provide exactly one of sourceAmount or targetAmount");
  }

  if (
    options.sourceAmount != null &&
    (!Number.isSafeInteger(options.sourceAmount) || options.sourceAmount <= 0)
  ) {
    throw new Error("sourceAmount must be a positive safe integer in sats");
  }
  return retryOnHashCollision(ctx, async () => {
    const swapParams = await ctx.deriveSwapParams();
    const hashLock = `0x${bytesToHex(swapParams.preimageHash)}`;
    const userId = bytesToHex(swapParams.userId);

    // The claiming address is this swap's own key, not the wallet-level
    // EVM address used for gasless deposits. A sponsored claim installs an
    // EIP-7702 delegation (Kernel) on the claiming address; keeping that
    // off the deposit address means Permit2 / EIP-2612 keep seeing a plain
    // EOA there. Recovery re-derives this key from the swap's index.
    const claimingAddress =
      claimingAddressOverride ?? deriveEvmAddress(swapParams.secretKey);

    const body = {
      hash_lock: hashLock,
      user_id: userId,
      claiming_address: claimingAddress,
      target_address: options.targetAddress,
      token_address: options.tokenAddress,
      evm_chain_id: options.evmChainId,
      amount_in: options.sourceAmount,
      amount_out: options.targetAmount?.toString(),
      referral_code: options.referralCode ?? ctx.referralCode,
      extra_fees: options.extraFees,
      promo_code: options.promoCode,
      gasless: options.gasless ?? true,
      bridge_target_chain: options.bridgeParams?.targetChain,
      bridge_target_token_address: options.bridgeParams?.targetTokenAddress,
      bridge_recipient_setup: options.bridgeParams?.recipientSetup,
    };
    const { data, error } = await ctx.apiClient.POST("/swap/spark/evm", {
      body,
    });

    if (error) {
      throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
    }
    if (!data) {
      throw new Error("No swap data returned");
    }

    assertSparkFunding(data, hashLock);
    if (
      options.sourceAmount != null &&
      BigInt(data.source_amount) !== BigInt(options.sourceAmount)
    ) {
      throw new Error(
        "Native Spark source amount differs from the requested amount",
      );
    }
    // The claim pays the target and signs with the claiming key: both must
    // be the ones this client sent.
    assertAddressEchoed(data, "target_evm_address", options.targetAddress);
    assertAddressEchoed(data, "client_evm_address", claimingAddress);

    // Store the swap if storage is configured
    await ctx.storeSwap(
      data.id,
      swapParams,
      {
        ...data,
        direction: "spark_to_evm",
      },
      options.targetAddress,
      {
        recipient: options.bridgeParams?.recipient,
        recipientWallet: options.bridgeParams?.recipientWallet,
      },
    );

    return { response: data, swapParams };
  });
}
