/**
 * One-shot submission of an Arkade offchain transaction.
 *
 * The client signs its checkpoints up front and sends them with the ark tx.
 * An arkd that supports one-shot submit finalizes in that single `submitTx`
 * call and hands the checkpoints back still carrying our signatures. An older
 * arkd strips them, returns server-signed checkpoints, and waits for
 * `finalizeTx` — so in that case we merge our signatures back in and finalize.
 */

import {
  assertSubmittedArkTxid,
  ConditionWitness,
  combineTapscriptSigs,
  getArkPsbtFields,
  matchServerCheckpoints,
  type OperatorProvider,
  setArkPsbtField,
  type Transaction,
} from "@arkade-os/sdk";
import { base64, hex } from "@scure/base";

/** The two operator calls a submission needs. */
export type OffchainSubmitProvider = Pick<
  OperatorProvider,
  "submitTx" | "finalizeTx"
>;

export interface OneShotSubmitResult {
  arkTxid: string;
  /**
   * `"one-shot"` when arkd finalized on submit; `"two-step"` when it was an
   * older arkd and we had to call `finalizeTx`.
   */
  mode: "one-shot" | "two-step";
}

/**
 * Submit a signed ark tx with fully user-signed checkpoints, finalizing
 * through `finalizeTx` only if arkd did not already finalize.
 *
 * @param provider operator provider
 * @param signedArkTx ark tx carrying every user-side signature it needs
 * @param checkpoints checkpoints as built (used to match arkd's response)
 * @param userSignedCheckpoints the same checkpoints, same order, carrying every
 *   user-side signature (and condition witness) they need
 */
export async function submitOffchainTxOneShot(
  provider: OffchainSubmitProvider,
  signedArkTx: Transaction,
  checkpoints: Transaction[],
  userSignedCheckpoints: Transaction[],
): Promise<OneShotSubmitResult> {
  if (userSignedCheckpoints.length !== checkpoints.length) {
    throw new Error(
      `expected ${checkpoints.length} user-signed checkpoints, got ${userSignedCheckpoints.length}`,
    );
  }
  const userByTxid = new Map(
    checkpoints.map((c, i) => [c.id, userSignedCheckpoints[i]] as const),
  );

  const response = await provider.submitTx(
    base64.encode(signedArkTx.toPSBT()),
    userSignedCheckpoints.map((c) => base64.encode(c.toPSBT())),
  );
  assertSubmittedArkTxid(response, signedArkTx, "submitTx");
  const { arkTxid } = response;
  const matched = matchServerCheckpoints(
    response.signedCheckpointTxs,
    checkpoints,
    "submitTx",
  );

  const userSignedFor = (local: Transaction): Transaction => {
    const user = userByTxid.get(local.id);
    if (!user) throw new Error(`no user-signed checkpoint for ${local.id}`);
    return user;
  };

  if (
    matched.every(({ server, local }) =>
      carriesTapscriptSigs(server, userSignedFor(local)),
    )
  ) {
    return { arkTxid, mode: "one-shot" };
  }

  const finalCheckpoints = matched.map(({ server, local }) => {
    const user = userSignedFor(local);
    combineTapscriptSigs(user, server);
    carryConditionWitness(user, server);
    return base64.encode(server.toPSBT());
  });
  await provider.finalizeTx(arkTxid, finalCheckpoints);
  return { arkTxid, mode: "two-step" };
}

/** Whether `server` already carries every tapscript sig `user` has on input 0. */
function carriesTapscriptSigs(server: Transaction, user: Transaction): boolean {
  const entries = (tx: Transaction) =>
    (tx.getInput(0).tapScriptSig ?? []).map(
      ([{ pubKey, leafHash }, sig]) =>
        hex.encode(pubKey) + hex.encode(leafHash) + hex.encode(sig),
    );
  const have = new Set(entries(server));
  const want = entries(user);
  return want.length > 0 && want.every((e) => have.has(e));
}

/** Copy our condition witness onto arkd's checkpoint if arkd dropped it. */
function carryConditionWitness(user: Transaction, server: Transaction): void {
  if (getArkPsbtFields(server, 0, ConditionWitness).length > 0) return;
  const [witness] = getArkPsbtFields(user, 0, ConditionWitness);
  if (witness) setArkPsbtField(server, 0, ConditionWitness, witness);
}
