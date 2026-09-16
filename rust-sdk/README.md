# satora-rust-sdk

Rust client for the Satora swap API. The crate is published as
`satora-rust-sdk`; its Rust library name is `lendaswap_sdk`.

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

| Operation | Rust support |
| --- | --- |
| Discovery | Health/status (including unhealthy HTTP 503), tokens, pairs, chain config, network fees, referral fee, MTP, support agents |
| Quotes | `/quote`, `/quote/lightning-send`, typed DEX quotes/calldata, and local BTC↔EVM/Solana composition |
| Create | All nine directions via `SwapCreation` and `create_swap_request`; generic EVM assets with explicit chain IDs |
| Inspect | All nine API swap directions through `get_swap_details`; compact `get_swap`; bulk status |
| Recovery | One-page `recover_swaps` and paginated `recover_all_swaps` |
| EVM funding | Optional `gasless` feature: external-wallet Permit2 execution, deposit probes/polling and ERC-4337 funding |
| Claim | EVM→Arkade and Lightning→Arkade; EVM targets via external/local wallets or server gasless submission |
| EVM refunds | Receipt-derived unilateral and cooperative refunds with owner/domain checks |
| Arkade wallet | Address, balance, send, boarding, settle |

Lightning sends accept `LightningDestination::Invoice`, `Address`, or `Lnurl`.
The `AddressSendMax` and `LnurlSendMax` variants specify a source amount instead
of an exact payout. Invoice-owned hash collisions return HTTP 409 without
retrying. Lightning→Arkade supports both source and target amounts, a UTF-8
invoice description of at most 639 bytes, and a per-swap surcharge.

`get_swap_details` preserves the server's full response. `get_swap` projects it
into payment instructions; its `receive_address` is empty for an EVM→Bitcoin
swap whose server response has no target address.

This is not full TypeScript SDK parity. Rust still lacks on-chain Bitcoin execution, complete Arkade refunds, EVM refund continuation, bridge orchestration; WebSocket tracking and contract managers. Reading/recovering those
swap records does not implement their execution flows. The TypeScript reference
is `../ts-sdk/packages/swap` and its underlying `../ts-pure-sdk` client.

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
unsigned calls for caller review and execution.

## Persistence and recovery

Use `storage_path` for a local SQLite database or provide an `Arc<dyn
SwapStorage>` with `storage`. The options are mutually exclusive. SQLite
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
Execution calls through cloned clients are serialized. Separate processes must
coordinate exclusive ownership of a swap before executing it.

`RpcWallet::connect` adapts a locally derived EVM key and checks the RPC chain.
`claim_evm_with_wallet` and `claim_evm_gasless` verify the funded HTLC, preimage,
contract version and remaining claim window before signing. Wallet-submitted
claims/refunds verify the exact terminal event in the mined receipt.
`refund_evm_with_wallet` derives the locked amount from the funding receipt and
checks chain time; `collab_refund_evm_with_wallet` validates the coordinator's
EIP-712 parameters and deposit ownership before authorizing a cooperative refund.
These operations require complete stored records and the relevant contract
addresses and output limits supplied by the caller.

## Build and test

This is a standalone crate, outside the root Cargo workspace. Use the repository's
pinned Rust toolchain; native builds need OpenSSL development libraries and
`protoc` for the Arkade dependencies.

```sh
cd client-sdk/rust-sdk
cargo test --locked
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
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
Submitted IDs and fees survive restarts; retries inspect the prior transaction
and reject a different unresolved spend. Only strict script version 1 is supported,
matching the current server and TypeScript SDK. Backends are trusted chain data
providers, not independent consensus validators.

Arkade `claim` now supports Bitcoin, EVM and Lightning inputs and spends all
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
settled inputs first. An expired intent with no observable settlement is left
unresolved for investigation; it is never silently replaced with new authorization.
The timeout refund path still requires the Arkade server, as in TypeScript.
`StrictVhtlcOptions` exposes all six script paths for wallet-level exit integrations.

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
