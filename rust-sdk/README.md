# satora-rust-sdk

Rust client for Satora swaps across Bitcoin, Arkade, Lightning and EVM chains.
The package name is `satora-rust-sdk`; the Rust library is `lendaswap_sdk`.
This crate is standalone and uses the repository’s pinned Rust toolchain.

## Usage

```rust,no_run
use lendaswap_sdk::{Client, types::{Address, QuoteAmount}};

# async fn example(mnemonic: String) -> Result<(), lendaswap_sdk::Error> {
let client = Client::builder()
    .mnemonic(mnemonic)
    .storage_path("swaps.sqlite")
    .build()?;

// Exact invoice amount; fees are deducted from the Arkade payout.
let swap = client.create_lightning_to_arkade_swap_with_options(
    QuoteAmount::Source(100_000),
    Address::Arkade("ark1…".into()),
    None,                  // optional extra fee in basis points
    Some("Order 42".into()),
).await?;

let details = client.get_swap_details(&swap.id).await?;
# let _ = details;
# Ok(())
# }
```

`Client::new(base_url)` creates a query-only client. The builder requires a
mnemonic, an xprv or wallet storage (see seed storage below). The default URL is `https://api.satora.io`. Amounts are in
source/target token smallest units; EVM amounts use `u128` inputs and decimal
strings on the wire, while Bitcoin amounts must be positive and fit in `i64`.

## Current coverage

| Operation                 | Rust support                                                                                                                                     |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| Discovery and quotes      | Health, tokens, pairs, chain/AA configuration, fees, MTP, referral/support data, typed DEX calls, exact local quote composition                  |
| Creation and recovery     | All nine swap directions, paginated recovery, SQLite snapshots and execution journals                                                            |
| EVM execution (`gasless`) | External and local wallets, Permit2 and EIP-2612, ERC-4337 and server funding/claims, timeout refunds, deposit recovery and refund continuation  |
| Bitcoin execution         | Strict version 1 Taproot claims/refunds, multiple outputs, Esplora and optional Electrum                                                         |
| Arkade execution          | All supported directions, offchain claims/refunds, cooperative refunds, delegated settlement, interrupted-batch recovery                         |
| Arkade wallet             | Balance, boarding, sends and settlement, journaled VHTLC funding, collaborative offboard with a send-all mode                                    |
| Bridges (`gasless`)       | CCTP v2 and USDT0 fees/messages, wallet/sponsored transfers, atomic inbound funding, outbound claims, delivery tracking and recovery             |
| Status feed (`websocket`) | Reconnect/resubscribe, bounded queues, HTTP reconciliation, cancellation                                                                         |
| Observations              | Bitcoin/Arkade/EVM HTLC readers, confirmation policy, verified preimages, reorg-aware bounded managers                                           |
| Actions and automation    | Chain-derived actions, persisted tracking, bounded workers, a stock auto-claimer, opt-in CCTP recovery, retries only of provably unsent attempts |

The reference implementations are `../ts-sdk/packages/swap` and
`../ts-pure-sdk`. The completed implementation checkpoints and scope decisions
are in [PARITY_PLAN.md](PARITY_PLAN.md). .NET bindings are outside this change.

| Creation route   | Client funding                   | Client settlement or recovery                     |
| ---------------- | -------------------------------- | ------------------------------------------------- |
| Bitcoin→Arkade   | Send BTC to the HTLC             | Claim Arkade; refund Bitcoin on failure           |
| Bitcoin→EVM      | Send BTC to the HTLC             | Claim EVM; refund Bitcoin on failure              |
| Arkade→EVM       | Send VTXOs to the VHTLC          | Claim EVM; refund Arkade on failure               |
| Arkade→Lightning | Send VTXOs to the VHTLC          | Server pays the invoice; refund Arkade on failure |
| Lightning→Arkade | Pay the returned invoice         | Claim Arkade                                      |
| Lightning→EVM    | Pay the returned invoice         | Claim EVM                                         |
| EVM→Bitcoin      | Wallet or sponsored HTLC funding | Claim Bitcoin; refund EVM on failure              |
| EVM→Arkade       | Wallet or sponsored HTLC funding | Claim Arkade; refund EVM on failure               |
| EVM→Lightning    | Wallet or sponsored HTLC funding | Server pays the invoice; refund EVM on failure    |

Examples are compiled templates: [routes.rs](examples/routes.rs) constructs every
route, [tracking.rs](examples/tracking.rs) wires observations and automation,
and [recovery.rs](examples/recovery.rs) illustrates settlement and restart recovery.
Their `main` functions do not create swaps or send payments.

Lightning sends accept `LightningDestination::Invoice`, `Address`, or `Lnurl`.
The `AddressSendMax` and `LnurlSendMax` variants specify a source amount instead
of an exact payout. `InvoiceExpecting` sends the payout amount alongside a
BOLT11 invoice, and the server rejects the swap if the invoice amount differs.
Invoice-owned hash collisions return HTTP 409 without retrying. Lightning→Arkade
supports both source and target amounts, a UTF-8 invoice description of at most
639 bytes, and a per-swap surcharge.

Lightning swaps are checked before anything is stored, and refused with
`InvalidSwap` otherwise. A Lightning→Arkade or Lightning→EVM hold invoice must pay
the hash of the client's preimage. An Arkade→Lightning or EVM→Lightning hash lock
must be the payment hash of the returned invoice, which must be the supplied one
when an invoice was supplied; for Lightning addresses and LNURLs the server resolves
the invoice, so only that agreement is checked. A supplied invoice that does not
parse is refused before the request. `recover_swaps` checks a page the same way
before storing any of it. `Client::send` with the creation request types skips
these checks.

`get_swap_details` preserves the server's full response. `get_swap` projects it
into payment instructions; its `receive_address` is empty for an EVM→Bitcoin swap
whose server response has no target address. `Swap::from` applies the same
projection to a `GetSwapResponse`. A `Swap` also carries its `direction`,
`created_at`, and under their wire names the refund locktimes and invoice expiry
in unix seconds. A locktime on the leg the client funds is when its refund
unlocks; on the leg the server funds it is the client's claim deadline.
`get_swap`, `get_swap_details`, `refresh_swap` and `wait_for_swap_status` refuse
with `InvalidSwap` a server response that names another swap than the one asked
for. They and `recover_swaps` refuse one that changes a stored swap's direction,
or whose invoice no longer pays a stored Lightning swap's hash lock, and the
stored copy stays as it was. A swap stored only as its key index, as in an
index-only store after a restart, gets the Lightning invoice and hash-lock checks
of `recover_swaps`, and `refresh_swap` stores nothing it refuses.

`create_swap_request(SwapCreation, SwapOptions)` provides explicit routes for all
nine directions. `EvmAsset` accepts a chain ID and either a named token or a
`TokenId::Other` contract address. Options include gasless execution, an external
EVM signer address, invoice descriptions, fees, and inbound/outbound bridge
routing. Bridge recipients and recipient setup are stored for later execution.
Creating a bridge route does not execute the bridge.

Bitcoin→Arkade requires an exact target amount and uses HASH160; other
client-owned locks use SHA256. `create_bitcoin_to_arkade_swap` takes that amount
as `sats_receive`. All new Bitcoin/Arkade scripts use version 1.
Malformed destinations, mismatched token chains, and invalid amounts are rejected
before allocating an index. Explicit reused-preimage errors retry at most ten
times, skipping ten additional indices per collision, matching TypeScript.
Invoice-owned locks never retry.

`get_quote` routes like the TypeScript `getQuote`. With `lightning_destination`
set, a Lightning payout from Arkade, Ethereum, Polygon, Arbitrum or native RBTC
is quoted by `/quote/lightning-send` with the provider's fee for that
destination. Otherwise a BTC to EVM route in either direction, or BTC to Solana,
is composed locally at 100 bps slippage. Other routes, and failures of either
step, go to the server's `/quote`, and its error is the one returned. Both
fallbacks log a warning through the client logger (see Logging), except for
routes composition reports as unsupported. `get_server_quote` sends only the `/quote` request. Without
explicit bridge fields it quotes a bridge-only chain (Base, Solana, USDT0
chains) on Arbitrum and names the real chain in `bridge_target_chain` or
`bridge_source_chain`. A request that sets either bridge field is sent as given.

`compose_quote(request, slippage_bps)` combines pair rates, network fees, pivot
configuration, referral fees and a live DEX quote. Its amount arithmetic uses
arbitrary-precision integers, including bridge-fee and peg-rate rounding.
`compose_quote_with_data` accepts pre-fetched discovery data. Routes it does not
cover return `Error::UnsupportedQuotePath`. Pair/config/referral
data caches for 60 seconds; `clear_quote_cache` invalidates it immediately.
DEX and network-fee requests always use fresh data. `get_dex_calldata` returns
unsigned calls that pay the output to `sender`, for caller review and execution.

## Persistence and recovery

Use `storage_path` for a local SQLite database or provide an `Arc<dyn SwapStorage>` with `storage`. The options are mutually exclusive. SQLite
partitions counters and swap key indices by the signing wallet's recovery xpub.
Independent connections/processes allocate indices transactionally. The parent
directory must exist and the filesystem must support SQLite locking.

The database stores key indices, complete swap responses, and local execution
metadata, without private keys or preimages. Back up both the
seed and database. The default in-memory store is ephemeral. Reopen the same
file with the same seed to resume swaps after a restart. If the database is lost,
call `recover_all_swaps(None)` before creating more swaps. Recovery advances the
counter without moving an existing counter backwards. Custom stores must
implement `ensure_next_key_index` to support recovery. Existing index-only SQLite
databases upgrade on open; refresh or recover swaps to populate their snapshots.
Custom stores retain index-only behavior until they implement the record methods.
An index-only store keeps records and their execution journal only for the life
of one `Client`. After a restart, `refresh_swap` and `recover_swaps` treat EVM
funding as possibly submitted and refuse it. If you know nothing was sent, clear it with
`update_swap_metadata(id, SwapMetadata { evm_fund_started: Some(false), ..Default::default() })`.
Deposit recoveries are not deduplicated across restarts on such a store.
Resuming submitted spends after a restart requires the record methods, except an
Arkade claim that arkd still holds (see Arkade execution).

Seed storage is opt-in, as in the TypeScript `WalletStorage`. `wallet_storage_path`
keeps the mnemonic in plaintext in its own SQLite file, created readable only by
its owner on Unix. It takes a plain file path: `:memory:` and SQLite `file:` URIs
are refused. Protect and back up that file like the seed itself.
`wallet_storage` accepts any `Arc<dyn WalletStorage>`, such as
`InMemoryWalletStorage`. The builder resolves the seed in this order. An xprv is
used as given and never stored, so combining it with wallet storage is an error.
A supplied mnemonic is saved to empty storage and refused if the storage holds a
different one, because this SDK's records hold no keys to survive an overwrite.
Otherwise the stored mnemonic is used, or a new 12-word mnemonic is generated and
saved before `build` returns. Processes that initialise the same file at once
agree on one mnemonic. With neither a seed nor wallet storage, `build` fails
instead of using a throwaway seed as TypeScript does. `Client::mnemonic` returns
the mnemonic, and a blank Arkade `identity_mnemonic` is filled with it. To load a
different mnemonic, store it with `WalletStorage::set_mnemonic`, build a new
client and drop the old clients and trackers. The file uses the TypeScript
`SqliteWalletStorage` table, so a TypeScript wallet file opens directly. Its
swaps are not in the `storage_path` database yet, so run `recover_all_swaps(None)`
before creating swaps. The `storage_path` database must be a different file and
still holds no private keys, preimages or seed.

Use `get_stored_swap` and `list_all_swaps` for offline snapshots, `refresh_swap`
for a persisted API refresh, and `update_swap_metadata` to merge local execution
details atomically. `delete_swap` and `clear_swap_storage` remove local records
without cancelling swaps or resetting the key allocation counter. Keep the seed
and recover from the server if you delete a record that still holds funds.

Swap creation allocates an index before making its HTTP request and persists the
swap ID after a successful response. If the process dies between those steps,
recover from the server. Successfully recovered pages remain persisted if a
later recovery page fails. Server recovery is subject to its gap/scan limits;
use `recover_swaps(Some(start_index))` for explicit deeper scans.

Swap/preimage and recovery derivation follow the TypeScript SDK. EVM deposit
keys in this Rust client use a separate per-swap derivation, so recovering a
TypeScript-created record does not transfer control of an external EVM wallet.
Records identify Rust per-swap, TypeScript BIP44, legacy swap-key, and external
EVM ownership explicitly. The original Rust derivation remains unchanged.

With the `gasless` feature, implement `evm::EvmWallet` to connect an external
wallet and use `fund_swap_with_wallet`. `FundingOptions` pins the HTLC,
coordinator, lock token, approved call targets, maximum input, and timeout.
Quotes are checked again after approval; signatures are recovered locally;
receipts must contain the matching HTLC creation event. Funding transaction IDs
and gasless operation hashes are persisted before receipt polling. A pending
submission prevents another funding attempt through that client/store.
Funding, refunds and stranded-token recovery reserve submission before sending,
including before a bundler or server can receive it. A lost response leaves an
unresolved reservation. Reconcile the original transaction with
`attach_evm_funding_transaction`, `attach_evm_refund_transaction`, or
`attach_evm_recovery_transaction`, then verify its receipt. If the application
proves nothing was sent, release the reservation by merging `Some(false)` into
`evm_fund_started`, `evm_refund_started` or `evm_recovery_started` with
`update_swap_metadata`. The tracker never releases these on its own;
`resolve_attempt` covers only the cases listed under automation. Retrying a known
refund verifies the receipt even after the coordinator deletes its deposit entry.
Each kind of execution through a `Client` and its clones, such as wallet spends or
swap claims, runs one at a time, and different kinds can overlap. Separate `Client`
instances over the same storage are not coordinated, so keep one `Client` per wallet
per process and clone it. Separate processes must coordinate exclusive ownership of
a swap before executing it.

`fund_swap_gasless_relay` funds a swap created with `gasless` through the server,
as the TypeScript `fundSwapGasless` does. The swap's EVM key signs a Permit2
transfer and, when the source token has no Permit2 allowance, an unlimited
EIP-2612 approval of Permit2; the server submits both and pays the gas. The
server's current copy of the swap must match the stored one, and the payload must
name the stored coordinator, which must also be the persisted one. For EVM to
Bitcoin and EVM to Lightning the payload must lock the stored `wbtc_address`; EVM
to Arkade responses name no lock token, so the payload's is signed unless one is
pinned. A pinned `FundingOptions` applies on top of these checks, never instead
of them: its contracts must equal the stored swap's, and it adds a call-target
allowlist and an input limit. Calls on the source token may only pay the relay
fee, capped by `RelayFundingOptions::max_relay_fee`, to the server or approve at
most the input, and the permit domain and nonce are read from the chain. The node
for these reads defaults to the chain's public RPC. A Kernel-delegated depositor
signs in Kernel's ERC-1271 envelope. The attempt is persisted in `evm_fund_relay`
before the request is sent, and every attempt for a swap signs the same Permit2
nonce, so at most one can fund it. After an error, the journal's outcome says what
to do. With no journal, or one at `Rejected`, `Reverted` or `Expired`, the call
can be repeated. At `Sending`, `Accepted` or `Uncertain` the server may have the
request: `reconcile_gasless_relay_funding` reports the swap funded, pending or
expired without sending anything, and expiry releases `evm_fund_started`. The
funding is recorded only once its receipt shows this swap's HTLC creation and
spends the nonce. A refused signature stays valid until its deadline, so until the
journal is `Funded` or `Expired` the wallet, sponsored and CCTP funding paths
refuse even if `evm_fund_started` is released, and the tracker's
`ProvenNotSubmitted` keeps it set.

`RpcWallet::connect` adapts a locally derived EVM key and checks the RPC chain.
`claim_evm_with_wallet` and `claim_evm_gasless` verify the funded HTLC, preimage,
contract version and remaining claim window before signing. Wallet-submitted
claims/refunds verify the exact terminal event in the mined receipt.
`refund_evm_with_wallet` derives the locked amount from the funding receipt and
checks chain time. A coordinator refund can be sent from any funded wallet and
pays the recorded depositor; a direct HTLC refund must come from the owner. EVM
cooperative refunds are intentionally unsupported by HTLC v6 and are not exposed
by the Rust SDK. Arkade cooperative refunds remain supported.
These operations require complete stored records and the relevant contract
addresses and output limits supplied by the caller.

## EVM recovery

Refund recovery APIs (`gasless` feature):

- `get_refunded_evm_swap_continuation` verifies the funding/refund receipts,
  transfer to the depositor, safe refund status, and current token balance.
- `continue_refunded_evm_swap` persists a replacement swap before wallet funding.
  For AA funding, compose `prepare_refunded_evm_swap_continuation` with
  `fund_swap_gasless`; the persisted replacement ID is reused after interruption.
- `recover_evm_deposit_with_wallet` transfers an explicit ERC-20 amount to an
  explicit destination, verifies the transfer receipt, and resumes a stored submission.
  Repeating the most recently settled recovery with identical parameters returns
  its receipt; any other parameters start a new transfer.
- `resume_gasless_funding` resolves a previously submitted UserOperation without
  sending it again. An operation receipt is not a substitute for verifying the
  HTLC funding event before proceeding with settlement.

Refresh the original swap before refund continuation. The latest recovery submission
is retained in storage; a failed or mismatched receipt requires explicit investigation.
Use one executor per swap across processes. Replacement creation and local storage
cannot be atomic across the server boundary: a crash immediately after the API
accepts creation may require swap recovery before attempting continuation again.

## Bitcoin execution

On-chain Bitcoin execution uses `bitcoin_htlc`, `claim_bitcoin`, and
`refund_bitcoin`. Configure an `EsploraBackend`, or enable `electrum` and connect
an `ElectrumBackend`. Both verify the configured network's genesis block and
reject observations spanning a tip change. `EsploraBackend` also reads Mempool on
an electrum backend, which serves no `/txs/chain`: history then pages its `/txs`
through `after_txid`, one request per ten transactions. The injectable
`BitcoinBackend` trait also supports private chain readers and offline integration
tests.

`BitcoinSpendOptions` requires a destination, confirmation policy, maximum fee,
and timeout. A `minimum_confirmations` of 0 claims against a mempool funding.
Refunds check chain height or median time past, never local wall clock. Claims
check the full expected amount before revealing the preimage.
Scripts must match the API address and the locally derived key/hash. The builder
supports multiple HTLC outputs, checks dust, and uses exact transaction weight.
With a store that keeps records, submitted IDs and fees survive restarts. Retries
resume from the HTLC address history: they return a pending or confirmed spend of
ours, replace a pending one only when an explicit `fee_rate` at least 1 sat/vB
above its rate is given, and sweep outputs that arrive after a confirmed spend. Backends without
`history` keep the journal behaviour and refuse a different unresolved spend. Only
strict script version 1 is supported, matching the current server and TypeScript
SDK. Backends are trusted chain data providers, not independent consensus
validators.

## Arkade execution

Arkade `claim` supports Bitcoin, EVM and Lightning inputs and spends all
eligible VTXOs after checking the expected funding amount. `refund_arkade` checks
maturity against Bitcoin chain data; `collab_refund_arkade` obtains and verifies
the receiver's signatures for Arkade→EVM and Arkade→Lightning refunds.
`arkade_htlc`, `vhtlc_balance`, and `spend_arkade` expose reconstruction, spend
observations, and an injectable `ArkadeBackend` for external integrations;
`amounts_for_swap` reads a stored swap's VHTLC amounts through the internal
wallet (TypeScript `amountsForSwap`). `ClientBuilder::arkade_config` has no
default endpoints; the TypeScript SDK defaults to `https://arkade.computer` on
mainnet and `https://mutinynet.arkade.sh` on signet (Mutinynet).

The expected amount is the target the swap was created with, which a refresh
never moves. When an EVM→Arkade source converts to less than quoted, the daemon
funds the VHTLC below that target. A claim, offchain or delegated, then reads the
swap owner's own EVM lock through the reader `Client::set_evm_reader` sets for
its chain (`gasless` feature) and accepts the VHTLC only if it covers the target
less the lock's shortfall, computed as the daemon does. The lock must be one this
wallet funded, through the coordinator its funding recorded. The shortfall is
measured against the lock the server said to expect, so a VHTLC further below the
target than `Client::set_conversion_shortfall_limit` allows (none by default) is
refused whatever the lock shows. Without a reader for the chain the claim is
refused. A record rebuilt from the server, by recovery or after a restart with an
index-only store, takes the server's amounts at that time as its targets.

The internal wallet's spends (`arkade_send`, `arkade_settle`, `arkade_offboard` and
`fund_arkade_swap`) take one lock shared by every clone of a `Client`, which
`arkade_offboard_quote` also waits for; claims and refunds never wait for it. Each
holds it for at most 120 seconds (`arkade_send`, `fund_arkade_swap`), 300 seconds
(`arkade_settle` and the quote) or 600 seconds (`arkade_offboard`, whose batch join
ark-client retries) and then fails with `Timeout`. A `Transport` or `Timeout` error
from `arkade_send`, `arkade_settle`, `arkade_offboard` or `fund_arkade_swap` leaves
the outcome unknown: check the balance or destination before sending or offboarding
again, settle again freely, and call `fund_arkade_swap` again, which resolves it from
its journal (each method's docs say more). Sends and offboards spend confirmed and
pre-confirmed VTXOs; recoverable ones need `arkade_settle`.

`fund_arkade_swap` pays an Arkade→Lightning or Arkade→EVM VHTLC. It finalizes the
wallet's pending transactions, refuses with `InvalidSwap` a swap that is not pending,
whose VHTLC holds outputs it did not journal, or whose VHTLC is locked to a server key
other than arkd's current signer, and journals its inputs and the VHTLC address before
paying and the Ark txid after. It also refuses a VHTLC amount below arkd's dust, which
ark-core would write as an OP_RETURN output, and a VHTLC whose hash lock is not the
swap's: the hash of the client's preimage for Arkade→EVM, the payment hash of the
stored invoice for Arkade→Lightning. `arkade_send` can spend inputs a failed funding
journaled; once arkd names and returns the send, which pays nothing to the VHTLC, the
next `fund_arkade_swap` resolves that journal.

A retry returns a journaled txid. Otherwise an Ark transaction arkd names as the
spender of any journaled input may be the payment, or that of an earlier attempt still
in flight when a retry widened the selection: the retry returns that transaction once
its VHTLC output is visible and fails with `UnresolvedFunding` until then, unless arkd
returns it and none of its outputs pays the VHTLC. A journaled input spent without
arkd naming a spender, a journaled input arkd does not know, and a journal for a VHTLC
address other than the swap's fail the same way and are never resolved automatically.
While a funding is unresolved, follow the swap's status, never pay the VHTLC by other
means, and give the swap up only once a check by hand shows where the funds went.
Inputs left unspent, forfeited into a batch, or spent by transactions that pay nothing
to the VHTLC prove the payment never happened, and the swap is paid again under the
same checks from a selection that includes every journaled input still spendable, so
arkd accepts at most one of the two payments if the first is still in flight. When no
journaled input is spendable any more, that rests on arkd also refusing inputs the
client sees as expired, swept, unrolled or under a retired signer. A swap that no
longer awaits funding instead has such a journal emptied before the refusal, so the
tracker stops waiting on its recovery. With a store that kept the swap's journal, that
refusal is the only error proving the wallet paid nothing; the others, such as a
refusal before the journal is read, leave an earlier payment possible. A swap
recovered on another device has no journal, and an index-only store keeps it only
for one `Client`'s life. Once arkd accepted the payment, a failure to journal its
txid is logged and the txid returned.

`arkade_offboard(address, amount)` joins a collaborative exit with the wallet's
confirmed and pre-confirmed VTXOs, the soonest-expiring first. Recoverable VTXOs and
boarding outputs need `arkade_settle` first: a batch that mixes an expired, unswept
VTXO with healthy ones fails for every participant, as arkd builds no forfeit
connector for it. For the same reason VTXOs that could expire before ark-client's
four batch join attempts end (each can wait out its intent's 120 s validity plus an
arkd session; at most the call's 600 s) stay out until they expire and turn
recoverable. On a server whose VTXOs live minutes, such as regtest's 512 s, that
margin is half the VTXO lifetime, read off the wallet's confirmed VTXOs since arkd's
info does not announce it, so a retried exit there can still meet an expiring input.
A wallet that has only received offchain gets the full margin until a settle or
boarding gives it a confirmed VTXO. VTXOs carrying assets stay offchain. The fee
comes from the server's intent fee program and out of change, as in ark-client, so
the address receives the amount exactly (TypeScript's `withdrawToL1` deducts it from
the amount).

Every exit keeps at least the server's dust (330 sats today) as Arkade change.
ark-client 0.11 registers a signing key for every exit but signs the batch's VTXO
tree only for one that creates a VTXO, so arkd would fail any batch that an exit
without change shares with others. `None` therefore sends everything except the
recoverable balance and the change, at the most that leaves at least dust (less
than dust plus one fee step when the server's fee program skips the amount that
leaves exactly dust). A fixed amount draws further VTXOs until the change reaches
dust and is refused when the wallet cannot keep it. `arkade_offboard_quote` prices
either form without sending. Address, dust and balance checks fail with
`InvalidSwap` before anything is sent (a short spendable balance names the sats
left out as expiring or recoverable), and so does a server fee program that charges
the inputs or the change, which ark-client's collaborative exit does not pay; a
`Transport` or `Timeout` error from `arkade_offboard` leaves its outcome unknown.

Offchain progress retains exact PSBTs with condition witnesses removed. Resume
with the same operation and destination (a claim whose submit was attempted
resumes whatever destination it pays): the SDK re-derives the preimage, queries
pending transactions after interrupted submission, and resumes checkpoint
finalization. Cosigner responses must preserve the locally constructed transaction
and contain valid signatures; receiver signatures survive Arkade's cosigning step.
Recoverable/expired VTXOs use delegated settlement. A `Storage` error once arkd may
hold a claim or refund leaves its outcome unknown, like `Transport` and `Timeout`:
repeat the call, which resumes from the journal.

A claim also finishes without its journal, after a lost database, on an index-only
store, or when another client with the same seed submitted it. When the indexer
shows VHTLC outputs spent offchain before the refund lock, `claim` asks arkd for
pending transactions over them and finalizes one that spends only those outputs in
the standard claim shape and carries a valid signature from the swap's receiver
key. Before the lock only the claim path spends the VHTLC offchain, so arkd
already holds the preimage and this query ignores `minimum_remaining_seconds`.
Past the lock the spend may be the server's refund, and the SDK refuses without
asking arkd. The receipt's `destination` is the address the claim pays, which may
differ from the one passed in. `continue_arkade_claim` finishes a persisted or
pending claim the same way but never submits a new one, like TypeScript's
`continueArkadeClaimSwap`. TypeScript instead asks arkd before every claim and
finalizes whatever it returns. The tracker reads a VHTLC spent offchain as
settled, so it never recommends this continuation; after restoring a lost database,
call `continue_arkade_claim` for Arkade-target swaps whose server leg shows spent
before their refund lock.

`settle_arkade` chooses offchain or delegated settlement from VTXO state and
resumes the persisted path. `delegate_arkade` exposes the batch path explicitly.
Pin the expected delegate public key, obtainable with `delegate_cosigner_key`.
`settle` does the same routing in one call with the configured Arkade wallet,
but trusts the cosigner key the API server reports (as the TypeScript SDK does);
`settle_arkade` or `delegate_arkade` with a pinned key is the stricter path.
Claims and cooperative/timeout refunds support mixed and swept VTXOs. Delegated
intent signatures use ALL; forfeits use ALL|ANYONECANPAY for the batch connector.
Timeout-refund forfeits carry the refund locktime and a non-final sequence.

A delegated request persists its exact intent before submission and records the
commitment transaction after success. After a lost response, retry reconciles
settled inputs first. A retried claim resubmits even inside
`minimum_remaining_seconds`, because the lost request may have delivered its
preimage already, unless the request failed to connect and so provably never left.
An offchain claim likewise finishes once its submit was attempted, reusing arkd's
pending entry when it has one and resubmitting otherwise. An intent lives
for about two minutes, so keep the margin above that. Two minutes after an intent
expires with its inputs unspent, the next call replaces it with a new intent for its
own request; the claim window is checked again unless an earlier claim may have
delivered the preimage. A late settlement of a replaced intent is still recognised
when the replacement spends the same inputs.
The timeout refund path still requires the Arkade server, as in TypeScript.
`StrictVhtlcOptions` exposes all six script paths for wallet-level exit integrations.

## Sponsored claims

`claim_evm_userop` publishes the same chain-verified claim through the local
EIP-7702 account with optional paymaster sponsorship. Supply reviewed DEX calls,
claim output limits, and a `UserOpConfig` with pinned EntryPoint/Kernel contracts.
The publisher verifies RPC chain ID and account ownership, then handles nonces,
EIP-7702 authorization, gas estimation, sponsorship, signing and submission.
`get_dex_calldata_fund` fetches the coordinator-recipient DEX call variant used by
this flow. Its calls still require caller review before signing.

Claim operation hashes are stored before receipt polling and prevent another
claim submission through any execution path. `resume_evm_claim_userop` discovers
the mined transaction after interruption. As with funding resumption, verify the
HTLC event before treating a bundler receipt as completed settlement. The local
publisher requires a key owned by the SDK seed; external accounts can use the
wallet-submitted claim API.

## Bridges

Bridge execution (`gasless` feature) uses `bridge::BridgeApi` for Circle and
LayerZero tracking, and `Client::{burn_cctp,mint_cctp,send_usdt0,recover_bridge_balance}`
for wallet or sponsored submissions. Pin both token and messaging contracts;
USDT0's OFT address is not necessarily its ERC20 address. CCTP fees use integer
arithmetic; OFT native fees require a fresh quote within an explicit spending cap.
CCTP Solana destinations accept decoded 32-byte ATA and wallet keys, including
Circle's ATA setup hook. Source signing currently supports EVM chains, matching
the reference SDK.

Bridge progress lives in `SwapMetadata::bridge_operations`, under `cctp_burn`,
`cctp_mint`, `usdt0_send`, and `bridge_sweep`. Intents are written before submitting;
transaction/UserOperation hashes are saved before receipt polling. Repeat the
same method and intent after a restart to verify its receipt without broadcasting
again. If a broadcast response was lost, reconcile the transaction externally and
use `attach_bridge_transaction`; an unresolved intent deliberately prevents an
automatic replacement. Supplied recovery hashes must identify the original
transaction (OFT events do not encode the destination recipient). Failed operations
remain available for inspection instead of authorizing another transfer.
A completed `bridge_sweep` repeated with a different recovery starts a new transfer.

`control::PollOptions` bounds the entire operation, including stalled network
requests, and supports a cloneable `Cancellation`. Cancellation stops local work;
it does not cancel an already broadcast transaction. Circle forwarding failures
retain an attestation for explicit mint recovery; expired attestations can be
renewed with `reattest_cctp`. Bridge receipts establish inclusion, not finality;
applications should apply their chain confirmation policy before treating a
payment as final. These paths have mocked execution and restart tests, but have
not been exercised against live bridges in this change.

`fund_swap_from_cctp` atomically combines a validated attestation's mint with
bounded Permit2 funding in a sponsored Kernel batch. It checks the destination
account, USDC token, net mint amount, coordinator, HTLC, approved DEX targets, and
funding receipt, and refuses a fresh submission once chain time reaches the swap
timelock. It also supports a nonce already consumed by the forwarder,
provided the account still holds the required USDC. An interrupted `cctp_fund`
operation blocks other funding paths until its original receipt is reconciled.

`recover_cctp_inbound` recovers a stalled CCTP-inbound burn from its source
transaction hash, with or without a swap record (TypeScript `recoverCctpInbound`).
It rebuilds the burn terms from the source receipt, where each `MessageSent` must
pair with a matching `DepositForBurn`, instead of using IRIS's first message. It
finds the seed key that controls the mint recipient: a stored swap's key, the
wallet key, the `key_index` hint, or the first 1000 per-swap and legacy keys. It
mints if the nonce is unused, then transfers to `recipient`. By default it moves
only the burn's net minted amount (`CctpSweep::Minted`). `CctpSweep::FullBalance`
moves the whole USDC balance, as TypeScript does; on an account that several swaps
share it can take USDC reserved for another swap's funding. Sponsored submission
batches the mint and transfer in one UserOperation; an external wallet uses Wallet
submission. Progress is journaled per burn under `cctp_recover:{domain}:{nonce}`
with `SwapStorage::put_bridge_journal`, and stores without it refuse. A repeat
returns the recorded result. A lost submission blocks until
`attach_cctp_recovery_transaction` or `release_cctp_recovery`. Resume in the
submission mode that started: the other mode refuses once a transfer, or an
unresolved wallet mint, is journaled. A completed wallet mint does not fix the
recipient or sweep of a transfer that was never sent. Recovery refuses while a
stored swap that owns the account, through a seed key or an external wallet, has
an unfinished `cctp_fund` or `cctp_mint`, or records funding that may have used
the burn's USDC. On an account one swap owns, that is any funding. On an account
several swaps share (the wallet key), it is a `cctp_fund` of this burn, or funding
after a `cctp_mint` of it. `fund_swap_from_cctp` then refuses the recovered burn,
so untrack a swap whose burn was recovered.

`claim_evm_to_bridge` appends CCTP or USDT0 adapter calls to reviewed DEX calldata
and returns the actual CCTP burn or LayerZero message query for delivery tracking.
Use `get_dex_calldata_fund` with the coordinator as sender; the server sets the
coordinator as recipient, and the DEX calls must enforce their own minimum output.
Bridge calls consume that output, so the coordinator's final sweep minimum is
zero. The adapter pulls the whole coordinator balance, so it gets an unlimited
approval that is reset to zero in the same batch, matching the daemon. The bridge minimum is only
checked on the claim receipt after it is mined, so the DEX calls' own minimum
output is what bounds the bridged amount. USDT0 native fees require an explicit
cap; excess passed to the deployed adapter stays with that adapter, as its
contract specifies. Outbound adapters currently use the repository's Arbitrum
deployments.

`usdt0_status_by_tx` and `wait_usdt0_by_tx` look a LayerZero message up by source
transaction hash alone (TypeScript `getLzMessageStatus` and `trackLzMessage`).
Unlike TypeScript they never pick the first of several messages: pass the GUID or
the destination EID. When only a claim's transaction hash is known,
`MessageQuery::from_receipt(&receipt, &OftRoute::arbitrum_adapter(eid))` recovers
the GUID from its receipt. Waiting retries read errors until the deadline and stops on statuses
that waiting cannot resolve, including `PAYLOAD_STORED`, which is delivered only
after someone retries `lzReceive` on the destination. `explorer_url` links to
LayerZero Scan.

A claim is reserved before its submission can leave the process, including
server-submitted and sponsored claims. A lost response prevents another claim
submission. Reconcile the original transaction with `attach_evm_claim_transaction`.
A server claim can be accepted before its relay names a transaction; if the server
later abandons it and the swap is server-funded again, release `evm_claim_started`
with `update_swap_metadata`. Bridge resumption verifies both the HTLC redemption and bridge receipt. Stored
bridge intent preserves recipient bytes without changing the original API-facing
EVM address or Solana base58 recipient.

## Status and observations

With the `websocket` feature, `Client::subscribe_status` starts a
`StatusSubscription`. Add UUIDs with `subscribe`, remove them with `unsubscribe`,
and receive `StatusEvent`s with `recv`. Shutdown is explicit and awaited;
dropping the handle cancels its workers. The source reconnects with bounded
backoff, resubscribes in batches, maintains heartbeats, and reconciles over HTTP
periodically and after reconnects. IDs beyond the socket's 256 limit continue
through HTTP and are promoted when slots become free. Events carry the
lowercase hyphenated swap ID, whatever spelling was subscribed. A slow consumer
receives `Lagged`, and the following HTTP reconciles re-send every status, at
most half the event queue per sweep. Unknown statuses and backwards transitions
are preserved as hints; neither this feed nor HTTP status proves settlement.

`observation` provides `BitcoinReader` (Esplora or Electrum), `ArkadeReader`, and
`EvmRpcReader`, combined through `ReaderSet` and `ContractManager`. References
include the HTLC's full identity and expected funding. `contract_references`
reconstructs them from stored swaps; configure the source coordinator and locked
EVM token when older API responses omit them. Specify an EVM starting block at
or before creation to bound historical scans. A server-funded Bitcoin leg has no
depth of its own. A manager bound with
`ContractManager::follow_bitcoin_min_confirmations` reads it at the client's
confirmation depth (see below), and `SwapTracker::new` binds its manager this
way; any other manager reads it at 0, like a TypeScript reader built without a
policy. Other legs take theirs from `ReferenceOptions`, which defaults to one:
the server does not act on a client funding until it is in a block. A reference
built by hand may carry its own Bitcoin depth, which wins over the client's.
Swaps with an EVM leg need the `gasless` feature for both the EVM reference and
`EvmRpcReader`. Without it `contract_references` rejects them instead of
returning a partial reference, because a missing leg reads as a Lightning leg.

Observers verify hash locks, funding amounts, spend inputs/scripts, EVM swap
keys, canonical log blocks, and revealed preimages. An unclassified spend stays
`SpentUnknown`. They do not latch terminal states across reorgs, except that a
running `EvmRpcReader` keeps logs more than 256 blocks below its last pass and so
misses a deeper reorg.
Bitcoin uses MTP/height and EVM uses block timestamps for expiry. Arkade funding
uses the Ark server's signed VTXO model; its refund clock comes from Bitcoin.
Readers trust the configured node/indexer rather than implementing consensus
validation. Missing, failed, or stale observations cannot authorize an action.
The manager bounds its cache, concurrency and read deadlines, emits changes,
supports targeted reconciliation, and owns an optional cancellable poller.

## Actions and tracking

`actions::derive_actions` is a pure policy function. It requires current chain
observations, explicit submission/bridge state, and chain clocks. It produces
fund, claim, cooperative/timeout refund, bridge recovery, wait, or terminal
actions. Funding is manual; refunds require application confirmation. Missing or
unknown observations never authorize a claim. Claim safety margins and expiry
grace periods are configurable through `ActionPolicy`.

A swap paid over Lightning has no client leg. It claims the server leg until the
claim margin before that leg's refund locktime, then waits on
`LightningResolution`, and ends as refunded once the chain reaches the locktime:
the preimage was never revealed, so the hold invoice unwinds on its own. That
terminal is not at risk, so it drops out of `refresh(true)`.

Such a swap also ends on the server status passed as `ActionInput::server_status`,
since the server never funds after setting one of these: `serverwontfund`,
`clientfundedtoolate`, `clientfundedserverrefunded` and `clientinvalidfunded` end
it as refunded, and `expired` ends it as expired. The chain still wins: a server
leg in the mempool keeps it waiting, a claimable one is claimed, and inside the
claim margin only the four refunded statuses end it before the locktime.
`clientfunded` only turns the wait into `ServerFunding`. Swaps with a client leg
ignore the status, and it never yields a fund, claim or refund action.

`tracker::SwapTracker` registers stored swaps with a dedicated `ContractManager`,
persists references/options/attempts, restores enabled records, and emits changed
actions through a bounded broadcast channel. Configure one reader per ledger
and EVM chain using `ReaderSet`. Use `contract_references` to reconstruct and
validate references; manually supplied references are trusted application input.

```rust,no_run
use std::sync::Arc;
use lendaswap_sdk::{Client, Result};
use lendaswap_sdk::observation::{ContractManager, ContractReader, ManagerOptions, ReferenceOptions};
use lendaswap_sdk::tracker::{SwapTracker, TrackerOptions, TrackingOptions};

# async fn track(client: Client, reader: Arc<dyn ContractReader>, id: &str, references: ReferenceOptions) -> Result<()> {
let manager = ContractManager::new(reader, ManagerOptions::default())?;
let tracker = SwapTracker::new(client.clone(), manager, TrackerOptions::default(), None)?;
let failures = tracker.restore().await?;
// Surface unsupported/corrupt records; they remain in storage for recovery.
assert!(failures.is_empty(), "tracking restore failures: {failures:?}");
// Stored swaps without a journal, such as one created just before a crash. The
// EVM scan start, sender and lock token in `references` belong to `id` alone. A
// failure (an unobservable leg, a full tracker) recurs until it is fixed.
let failures = tracker
    .adopt_untracked(|stored| {
        let own = stored.response.id() == id;
        let references = if own { references.clone() } else { references.shared_policy() };
        Some((references, TrackingOptions::default()))
    })
    .await?;
for (id, error) in failures {
    tracing::warn!(%id, %error, "swap not adopted for tracking");
}
// A retired swap keeps its journal and is resumed; a swap adoption skipped is
// tracked directly.
if tracker.record(id).await.is_err() {
    if client.get_stored_swap(id)?.is_some_and(|s| s.metadata.tracking.is_some()) {
        tracker.resume(id).await?;
    } else {
        let contracts = client.contract_references(id, &references)?;
        tracker.track(id, contracts, TrackingOptions::default()).await?;
    }
}
let mut events = tracker.events();
let worker = tracker.start()?;
match events.recv().await {
    Ok(event) => println!("{event:?}"),
    Err(_) => tracker.hint_all()?, // reconcile after a slow consumer misses events
}
worker.shutdown().await?;
# Ok(())
# }
```

`ClientBuilder::bitcoin_min_confirmations` sets the confirmations a server-funded
Bitcoin HTLC needs before it is claimable, like the TypeScript
`withBitcoinMinConfirmations`. The default is 0: the claim goes out as soon as the
funding reaches the mempool, which trusts the funder not to double-spend it
(claiming publishes the preimage, so a funder that then replaced its funding could
take both legs). Set 1 or more for a block-depth policy that does not rely on the
funder's good behaviour. `Client::set_bitcoin_min_confirmations` changes it later,
with `None` reading as 0. Every clone of the client shares it and the tracker reads
it on every chain read, so a change applies to swaps already tracked. The depth
gates the funding only: a claim or refund of the HTLC counts as soon as it is
seen, as in TypeScript. Nothing pushes a new block, so while the depth is above
one the at-risk refresh runs at least every 15 seconds and the claim follows the
block that satisfied it. Journals written by earlier versions load unchanged,
except that their server-funded Bitcoin leg now follows the client's depth. One
`ContractManager` serves the trackers of one client; `SwapTracker::new` refuses a
manager that already follows another.

`adopt_untracked` skips a swap whose last stored server status is settled, as the
TypeScript client does. Refresh the swap first, or track it directly, to watch it
regardless.

Use `status_hint` to feed WebSocket events into the tracker, or `hint_status` for
a status obtained another way. Both trigger chain reads, and the last hinted
status, or else the stored response status, becomes `server_status` for the
Lightning terminals above. The journal keeps the last hinted status, so a terminal
it produced survives a restart and still retires on schedule instead of starting
over. `hint` coalesces IDs, `refresh(true)` reconciles at-risk
swaps, and periodic full refreshes also revisit apparently completed swaps after
reorgs. `actions` returns the last published snapshot; use `reconcile` for a fresh
read before acting. `untrack` disables a record without deleting its journal;
`resume` re-enables it. A swap whose terminal outcome has held for
`TrackerOptions::retire_after` (default 24 hours) is disabled as if by `untrack`,
which frees its `max_swaps` slot; afterwards `record` and `actions` report that it
is not tracked, and `resume` re-enables it with a fresh window. An expired swap
whose bridge is still `CctpBurn` needs a manual mint, so it stays tracked until
`update_options` records the bridge as delivered. A retired swap is
no longer watched, so a reorg deeper than the window, or a deposit made after an
unfunded swap expired, goes unnoticed until it is resumed. Dropping the worker
cancels local work.

Automatic execution needs both an `ActionExecutor` and per-swap `auto_claim` or
`auto_cctp` opt-in. No executor is configured by default, and funding/refunds are
never automated.

`tracker::AutoClaimer` is the stock claim executor, the counterpart of the
TypeScript `withAutoClaim`. It claims Arkade targets through the client's Arkade
config (offchain, or delegated when outputs are recoverable), EVM to Bitcoin swaps
with `AutoClaimConfig::bitcoin` (at the confirmation depth the tracker waited for,
at the backend's fee estimate capped by `maximum_fee`), and EVM targets with
`AutoClaimConfig::evm`. The destination is the one recorded when the swap was
created and must match the server's; a swap created elsewhere needs
`AutoClaimConfig::destination`. An EVM claim signs only for an (HTLC, coordinator)
pair in its chain's `deployments`. The server does not publish these, so the
application supplies its current and legacy deployments. A direct payout uses a
sponsored UserOperation when the chain has `user_op`, and the server relay
otherwise, or when that operation sent nothing. A payout through the relay, and a
DEX payout, must reach `target_amount` less the daemon's slippage (1%, or 2% for
Bitcoin to EVM), and a direct one through the relay must come with no server
calls. A DEX payout on Arbitrum needs `user_op`. Bridged payouts are refused,
because the daemon builds their calls with no minimum output; claim them with a
reviewed plan through `claim_evm_to_bridge`. Keep `AutoClaimConfig::timeout` (90
seconds) below `TrackerOptions::execution_timeout`; `SwapTracker::new` checks it.

Other executors should call the SDK's durable execution methods with
application-approved addresses, limits and bridge plans. Supply bridge state from
verified receipts, attestations and delivery queries via `update_options`; server
swap status does not establish bridge delivery.

An attempt is persisted before invoking the executor. It may return `Retryable`, or
`NotSent` with a reason that is emitted as an error event, only if nothing was
submitted. `AutoClaimer` does so only while the rail's submission journal is still
clear, since every rail writes it just before a submission can leave the process.
Such an attempt runs again, after a fresh chain read, once `retry_delay` (2
seconds) has passed, with the delay doubling up to `retry_delay_max` (30 seconds).
After `max_attempts` (6) runs the action turns `Manual`; a different recommended
action starts its own count. `SwapActions::needs_user` is true for a confirmation,
a manual step or `SubmissionRecovery`; filter tracker events on it where the
TypeScript client uses `onActionRequired`. Unchanged actions are not emitted
again, so after a lagged receiver read `actions` for each tracked swap.
Errors, timeouts, cancellation and lost responses remain unresolved until the
application reconciles the rail journal/chain and calls `resolve_attempt`.
`Submitted` waits for observed settlement; it never declares success by itself.
`ProvenNotSubmitted` gives the action a fresh attempt budget and releases only the
attempted action's unsent reservations (the claim flag; for CCTP recovery,
hashless `cctp_fund`/`cctp_mint` entries and a hashless funding flag). Recorded
transactions are always kept. The proof must also cover any funding sent outside
the tracker since the attempt started.
Use one tracker and exclusive execution ownership per store across processes.

## Logging

The SDK emits the TypeScript SDK's structured events for Arkade claims, refunds
and delegated settlement, failed creation attempts, recovery requests, the two
`get_quote` fallbacks and the relay's depositor code lookup, with TypeScript's
event names, modules, operations and levels. The `continue_*` claim
events appear only when a claim resumes or finishes one arkd already holds, since
Rust does not query arkd before every claim. `arkade.settle.cosignerKeyFallback`,
`client.refundSwap.serverLookupFailed`,
`client.reconcileGaslessRelay.serverLookupFailed` and
`client.claim.sponsoredFallback` are Rust-only. Without a custom logger
they go to `tracing` under the `lendaswap_sdk` target, so a subscriber filtering
on `lendaswap_sdk=debug` shows them. With no subscriber they reach a `log` crate
logger such as `env_logger` through tracing's `log` fallback, like the SDK's other
`tracing` events, and with neither there is no output.
`ClientBuilder::logger` replaces that sink with any `Logger`, including a closure
taking `&LogRecord`, and like TypeScript stays silent until `log_level` is set.
`Client::set_logger` does the same after `Client::new`.

A `LogRecord` carries the event, module, operation, swap ID, error text and a
`data` map with TypeScript's camelCase keys and decimal-string amounts. Values
under secret-named keys (`preimage`, `userSecretKey`, `mnemonic`, `signature`,
`api_key` and the rest of TypeScript's list) are redacted, and no event carries a
preimage or private key. `client.recover.start` logs the wallet's recovery xpub
at info, as TypeScript does. The logger runs synchronously and must not panic.
There is no console logger: use `TracingLogger` with a `tracing_subscriber::fmt`
subscriber. TypeScript's Electrum/Esplora fallback, price, CCTP preflight,
funding-record and collaborative-refund fallback events have no Rust
counterpart, because those fallbacks do not exist here. Request spans and other
existing `tracing` events stay on `tracing` whichever logger is set.

## Compatibility and validation limits

- Strict Bitcoin/Arkade script version 1 matches the current server. Arkade
  timeout refunds still involve the Ark server; a wallet-wide unilateral exit
  service is not part of either swap SDK.
- Sponsored accounts use Rust's local-owner EIP-7702 model. TypeScript's external
  factory smart-account model is not interchangeable. External wallets can use
  direct transaction execution; their keys are never guessed from the seed.
- EVM execution and bridges currently share the historical `gasless` feature
  flag, including direct wallet execution. `websocket` and `electrum` are
  independently optional. Observations/actions/storage compile by default;
  tracking swaps with an EVM leg needs `gasless`.
- Confirmation policy belongs to the application. Configured RPCs, indexers,
  Ark servers and bridge APIs remain data dependencies; this SDK is not a full
  consensus or SPV validator. Ambiguous Arkade batch spends remain unknown.
- Validation includes local mocks, deterministic vectors and real local-EVM
  execution. Live BTCPayServer integration, production bundler/paymaster
  sponsorship, Circle/LayerZero delivery, and production Arkade batch settlement
  still need deployment testing.
- Arkade dependencies use the published 0.11.0 crates, which remove the yanked
  dependency that previously required Git pins.

## Build and test

This is a standalone crate, outside the root Cargo workspace. Use the repository's
pinned Rust toolchain; native builds need OpenSSL development libraries and
`protoc` for the Arkade dependencies.

```sh
cd client-sdk/rust-sdk
cargo test --locked
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo check --locked --no-default-features --features websocket,electrum
cargo doc --locked --all-features --no-deps
cargo +nightly-2025-11-01 fmt --check
```

Tests use local HTTP/RPC mocks, SQLite reopen/concurrency checks, signer vectors,
and the pinned OpenAPI schema. Live tests are ignored by default:

```sh
LENDASWAP_API_URL=http://localhost:3333 cargo test --test integration_live -- --ignored
```

Both SDKs pin the same `openapi.json`. Refresh it from the repository root with
`just export-openapi-sdk`, then run schema and client tests. API requests include
`X-Lendaswap-Client` and one protocol epoch header per component
(`x-satora-bitcoin-htlc-version` and the others in `client.rs`).

For real EVM execution tests, run `scripts/e2e/test-rust-sdk-local.sh` from the
repository root (Rust, Foundry and native build dependencies required). The suite
starts isolated Anvil nodes, deploys the repository contracts, and verifies claims,
Permit2 funding/refunds, restart recovery, tracker execution and reorgs. Swap API
snapshots/quotes remain fixtures; full-daemon coverage uses the separate Rust
process driver. See `scripts/e2e-rust/README.md` for setup and coverage boundaries.
