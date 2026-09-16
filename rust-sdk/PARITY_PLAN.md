# Rust SDK parity implementation

Reference: TypeScript SDK sources at repository commit `cf8c0b90`, including
`client-sdk/ts-sdk/packages/swap` and the underlying `client-sdk/ts-pure-sdk`.
Issue: https://github.com/satoraHQ/swap/issues/1063

Scope: implement equivalent Rust workflows for the remaining swap SDK features.
Public APIs may use idiomatic Rust types. Existing Rust key derivation must remain
stable; imported TypeScript swaps must identify the signer they require. .NET
bindings and the independent escrow packages are outside this change.

Each numbered step is a separate commit after formatting, Clippy, and relevant
tests pass. Update this checklist in the implementation commit. Commit descriptions
state the actual change and validation, without tool attribution.

## Completed baseline

- [x] `3c530f5f`: current Lightning API shapes; all nine response directions;
  EVM→Lightning creation/quotes; discovery; SQLite indices; recovery; bounded
  polling; redacted diagnostics. Includes HASH160 recovery for BTC→Arkade.

## Remaining steps

1. [ ] **Persist complete swap records and execution metadata.** Extend memory and
   SQLite stores with list/get/update/delete/clear, preserving allocation counters.
   Retain original key indices, bridge recipients, deposit transaction data, and
   response snapshots across restarts. Add explicit key ownership/derivation data
   for imported/external EVM signers. Test migration, monotonic allocation,
   concurrent writes, reopening, and interrupted recovery.

2. [ ] **Complete swap creation and routing.** BTC→Arkade; Bitcoin/Arkade/Lightning→EVM;
   EVM→Bitcoin; generic EVM tokens with explicit chain IDs; bridge options;
   invoice descriptions and fee overrides. Extend the general dispatcher without
   ambiguous BTC rail inference. Match strict script versions, HASH160/SHA256,
   amount encoding, retries, and destination validation to TypeScript. Validate
   request bodies and response handling for every direction against OpenAPI.

3. [ ] **Add DEX quoting and local quote composition.** Typed DEX quote/calldata
   requests and responses, token discovery, explicit quote amount variants,
   fee/referral composition, source/target amount rounding, peg conversions,
   bridge fees and cache expiry. Reuse TypeScript test vectors and cover large
   ERC-20 amounts without floating-point amount loss.

4. [ ] **Complete EVM execution.** Typed calldata retrieval; external wallet
   signer interface; Permit2 funding and approvals; EIP-712 claims; gasless claim;
   unilateral/cooperative refunds; refunded-swap continuation; stranded deposit
   recovery. Validate chain IDs, recipients, HTLC versions, hash locks, nonces,
   typed-data domains, and submitted transactions before releasing a preimage or
   signing. Compare signatures/calldata against TypeScript/contract vectors.

5. [ ] **Implement on-chain Bitcoin claims and refunds.** Strict and legacy HTLC
   reconstruction, UTXO observation through Esplora/Electrum, minimum confirmations,
   network/address checks, timelocks, fee estimation, transaction construction,
   signing, broadcast, and recovery. Test golden scripts/transactions, invalid
   script parameters, insufficient funding, fees/dust, and premature refunds.

6. [ ] **Complete Arkade claims and refunds.** BTC→Arkade claim support, VHTLC
   balances and receipt detection, cooperative and unilateral refunds,
   delegate/cosigner flows, and continuation after interrupted claims. Reuse
   strict VHTLC vectors and validate expected keys, script/address, network,
   amounts, spend state, and preimages before signing or submission.

7. [ ] **Implement bridge orchestration.** CCTP domains/tokens and fee estimates,
   burn/approval/permit/user-operation construction, attestation polling,
   inbound funding and recovery, outbound forwarding and recipient persistence;
   USDT0/LayerZero routing, fees, message tracking and recovery. Test fixtures for
   each supported route, pending/failed/complete states, retries, and restart
   resumption. External providers must remain injectable in offline tests.

8. [ ] **Add status subscriptions.** WebSocket connect/subscribe/unsubscribe,
   event decoding, reconnect/resubscribe, bounded queues, explicit shutdown,
   cancellation, and HTTP reconciliation. Test a local WebSocket server with
   disconnects, duplicate/out-of-order updates, malformed payloads, and shutdown.

9. [ ] **Implement contract observations and managers.** Typed HTLC references,
   consistent keys, Bitcoin/Arkade/EVM readers, preimage extraction/validation,
   chain-verified funding/spend/expiry observations, reorg/confirmation behavior,
   and cancellation-safe manager lifecycles. Port TypeScript observation vectors.

10. [ ] **Implement action tracking and optional automation.** Derive funding,
    claim, cooperative/unilateral refund, bridge recovery, wait, and terminal
    actions from observations; persist/resume tracked swaps; expose action
    subscriptions; reconcile at-risk swaps; optional auto-claim with bounded
    concurrency and failure recovery. Port action-state tests and exercise full
    mocked create→fund→claim/refund→terminal lifecycles.

11. [ ] **Finish public API/documentation and integration verification.** Export
    supported entry points; provide examples for each route and recovery path;
    update the coverage table and migration notes; check package contents,
    default and optional feature builds, documentation, and the full test suite.
    Keep a precise list of live network/chain validations that were not executed.

## Validation gates

From `client-sdk/rust-sdk`:

```sh
cargo +nightly-2025-11-01 fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked --all-features
git diff --check
```

Focused regression suites may run during implementation. A committed step must
pass the applicable gates and record meaningful behavior tests, not only type
compilation. Broadcasting tests use local mocks or explicitly configured local
chains; no production payments are part of this work.
