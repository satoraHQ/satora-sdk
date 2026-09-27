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
mnemonic or xprv. The default URL is `https://api.satora.io`. Amounts are in
source/target token smallest units; EVM amounts use `u128` inputs and decimal
strings on the wire, while Bitcoin amounts must be positive and fit in `i64`.

## Current coverage

| Operation                 | Rust support                                                                                                                                    |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| Discovery and quotes      | Health, tokens, pairs, chain/AA configuration, fees, MTP, referral/support data, typed DEX calls, exact local quote composition                 |
| Creation and recovery     | All nine swap directions, paginated recovery, SQLite snapshots and execution journals                                                           |
| EVM execution (`gasless`) | External/local wallets, Permit2 and EIP-2612, ERC-4337 funding/claims, server claims, timeout refunds, deposit recovery and refund continuation |
| Bitcoin execution         | Strict version 1 Taproot claims/refunds, multiple outputs, Esplora and optional Electrum                                                        |
| Arkade execution          | All supported directions, offchain claims/refunds, cooperative refunds, delegated settlement, interrupted-batch recovery                        |
| Bridges (`gasless`)       | CCTP v2 and USDT0 fees/messages, wallet/sponsored transfers, atomic inbound funding, outbound claims, delivery tracking and recovery            |
| Status feed (`websocket`) | Reconnect/resubscribe, bounded queues, HTTP reconciliation, cancellation                                                                        |
| Observations              | Bitcoin/Arkade/EVM HTLC readers, confirmation policy, verified preimages, reorg-aware bounded managers                                          |
| Actions and automation    | Chain-derived actions, persisted tracking, bounded workers, opt-in automatic claims/CCTP recovery, conservative retry journals                  |

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
of an exact payout. Invoice-owned hash collisions return HTTP 409 without
retrying. Lightning→Arkade supports both source and target amounts, a UTF-8
invoice description of at most 639 bytes, and a per-swap surcharge.

`get_swap_details` preserves the server's full response. `get_swap` projects it
into payment instructions; its `receive_address` is empty for an EVM→Bitcoin
swap whose server response has no target address.

`create_swap_request(SwapCreation, SwapOptions)` provides explicit routes for all
nine directions. `EvmAsset` accepts a chain ID and either a named token or a
`TokenId::Other` contract address. Options include gasless execution, an external
EVM signer address, invoice descriptions, fees, and inbound/outbound bridge
routing. Bridge recipients and recipient setup are stored for later execution.
Creating a bridge route does not execute the bridge.

Bitcoin→Arkade requires an exact target amount and uses HASH160; other
client-owned locks use SHA256. All new Bitcoin/Arkade scripts use version 1.
Malformed destinations, mismatched token chains, and invalid amounts are rejected
before allocating an index. Explicit reused-preimage errors retry at most ten
times, skipping ten additional indices per collision, matching TypeScript.
Invoice-owned locks never retry.

`compose_quote(request, slippage_bps)` combines pair rates, network fees, pivot
configuration, referral fees and a live DEX quote. Its amount arithmetic uses
arbitrary-precision integers, including bridge-fee and peg-rate rounding.
`compose_quote_with_data` accepts pre-fetched discovery data. Unsupported paths
can use `get_quote`; automatic fallback is left to the caller. Pair/config/referral
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
Resuming submitted spends and claims after a restart requires the record methods.

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
Execution calls through cloned clients are serialized. Separate processes must
coordinate exclusive ownership of a swap before executing it.

`RpcWallet::connect` adapts a locally derived EVM key and checks the RPC chain.
`claim_evm_with_wallet` and `claim_evm_gasless` verify the funded HTLC, preimage,
contract version and remaining claim window before signing. Wallet-submitted
claims/refunds verify the exact terminal event in the mined receipt.
`refund_evm_with_wallet` derives the locked amount from the funding receipt and
checks chain time. EVM cooperative refunds are intentionally unsupported by HTLC
v6 and are not exposed by the Rust SDK. Arkade cooperative refunds remain supported.
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
- `resume_gasless_funding` resolves a previously submitted UserOperation without
  sending it again. An operation receipt is not a substitute for verifying the
  HTLC funding event before proceeding with settlement.

Refresh the original swap before refund continuation. Each recovery submission is
retained in storage; a failed or mismatched receipt requires explicit investigation.
Use one executor per swap across processes. Replacement creation and local storage
cannot be atomic across the server boundary: a crash immediately after the API
accepts creation may require swap recovery before attempting continuation again.

## Bitcoin execution

On-chain Bitcoin execution uses `bitcoin_htlc`, `claim_bitcoin`, and
`refund_bitcoin`. Configure an `EsploraBackend`, or enable `electrum` and connect
an `ElectrumBackend`. Both verify the configured network's genesis block and
reject observations spanning a tip change. The injectable `BitcoinBackend` trait
also supports private chain readers and offline integration tests.

`BitcoinSpendOptions` requires a destination, confirmation policy, maximum fee,
and timeout. Refunds check chain height or median time past, never local wall
clock. Claims check the full expected amount before revealing the preimage.
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
the receiver's signatures for Arkade→EVM refunds. `arkade_htlc`, `vhtlc_balance`,
and `spend_arkade` expose reconstruction, spend observations, and an injectable
`ArkadeBackend` for external integrations.

Offchain progress retains exact PSBTs with condition witnesses removed. Resume
with the same operation and destination: the SDK re-derives the preimage, queries
pending transactions after interrupted submission, and resumes checkpoint
finalization. Cosigner responses must preserve the locally constructed transaction
and contain valid signatures; receiver signatures survive Arkade's cosigning step.
Recoverable/expired VTXOs use delegated settlement.

`settle_arkade` chooses offchain or delegated settlement from VTXO state and
resumes the persisted path. `delegate_arkade` exposes the batch path explicitly.
Pin the expected delegate public key, obtainable with `delegate_cosigner_key`.
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
for about two minutes, so keep the margin above that. An expired intent with no observable settlement is left
unresolved for investigation; it is never silently replaced with new authorization.
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
or before creation to bound historical scans. Confirmation requirements default
to one on both legs; accepting zero-confirmation Bitcoin funding is explicit.
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
let contracts = client.contract_references(id, &references)?;
let manager = ContractManager::new(reader, ManagerOptions::default())?;
// A retired swap keeps its journal; `resume` it rather than tracking it again.
let journaled = client
    .get_stored_swap(id)?
    .is_some_and(|swap| swap.metadata.tracking.is_some());
let tracker = SwapTracker::new(client, manager, TrackerOptions::default(), None)?;
let failures = tracker.restore().await?;
// Surface unsupported/corrupt records; they remain in storage for recovery.
assert!(failures.is_empty(), "tracking restore failures: {failures:?}");
if !journaled {
    tracker.track(id, contracts, TrackingOptions::default()).await?;
} else if tracker.record(id).await.is_err() {
    tracker.resume(id).await?;
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

Use `status_hint` to feed WebSocket events into the tracker; status values only
trigger chain reads. `hint` coalesces IDs, `refresh(true)` reconciles at-risk
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

Automatic execution needs both an `ActionExecutor` adapter and per-swap
`auto_claim` or `auto_cctp` opt-in. Adapters should call the SDK's durable execution
methods with application-approved addresses, limits and bridge plans. Supply
bridge state from verified receipts, attestations and delivery queries via
`update_options`; server swap status does not establish bridge delivery. No
executor is configured by default, and funding/refunds are never automated.

An attempt is persisted before invoking the adapter. A callback must explicitly
return `Retryable` only if nothing was submitted; retries have backoff and a cap.
Errors, timeouts, cancellation and lost responses remain unresolved until the
application reconciles the rail journal/chain and calls `resolve_attempt`.
`Submitted` waits for observed settlement; it never declares success by itself.
`ProvenNotSubmitted` releases only the attempted action's unsent reservations
(the claim flag; for CCTP recovery, hashless `cctp_fund`/`cctp_mint` entries and a
hashless funding flag). Recorded transactions are always kept. The proof must also
cover any funding sent outside the tracker since the attempt started.
Use one tracker and exclusive execution ownership per store across processes.

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
`X-Lendaswap-Client` and `x-satora-server-version` compatibility headers.

For real EVM execution tests, run `scripts/e2e/test-rust-sdk-local.sh` from the
repository root (Rust, Foundry and native build dependencies required). The suite
starts isolated Anvil nodes, deploys the repository contracts, and verifies claims,
Permit2 funding/refunds, restart recovery, tracker execution and reorgs. Swap API
snapshots/quotes remain fixtures; full-daemon coverage uses the separate Rust
process driver. See `scripts/e2e-rust/README.md` for setup and coverage boundaries.
