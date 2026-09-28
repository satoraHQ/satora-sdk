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

1. [x] **Persist complete swap records and execution metadata.** Extend memory and
       SQLite stores with list/get/update/delete/clear, preserving allocation counters.
       Retain original key indices, bridge recipients, deposit transaction data, and
       response snapshots across restarts. Add explicit key ownership/derivation data
       for imported/external EVM signers. Test migration, monotonic allocation,
       concurrent writes, reopening, and interrupted recovery.

2. [x] **Complete swap creation and routing.** BTC→Arkade; Bitcoin/Arkade/Lightning→EVM;
       EVM→Bitcoin; generic EVM tokens with explicit chain IDs; bridge options;
       invoice descriptions and fee overrides. Extend the general dispatcher without
       ambiguous BTC rail inference. Match strict script versions, HASH160/SHA256,
       amount encoding, retries, and destination validation to TypeScript. Validate
       request bodies and response handling for every direction against OpenAPI.

3. [x] **Add DEX quoting and local quote composition.** Typed DEX quote/calldata
       requests and responses, token discovery, explicit quote amount variants,
       fee/referral composition, source/target amount rounding, peg conversions,
       bridge fees and cache expiry. Reuse TypeScript test vectors and cover large
       ERC-20 amounts without floating-point amount loss.

4. [x] **Complete EVM execution.** Typed calldata retrieval; external wallet
       signer interface; Permit2 funding and approvals; EIP-712 claims; gasless claim;
       timeout refunds; refunded-swap continuation; stranded deposit
       recovery. Validate chain IDs, recipients, HTLC versions, hash locks, nonces,
       typed-data domains, and submitted transactions before releasing a preimage or
       signing. Compare signatures/calldata against TypeScript/contract vectors.
       Funding checkpoint: external-wallet interface, bounded Permit2 approvals,
       fresh-quote validation, receipt checks, transaction/UserOperation persistence,
       and recovered-key selection implemented.
       Settlement checkpoint: RPC wallet, chain-verified EVM claims, server gasless
       claims and receipt-derived timeout refunds implemented. EVM cooperative
       refunds are intentionally unsupported and excluded from the SDK.
       Recovery checkpoint: receipt-verified refunded-token continuation, persisted
       replacement IDs, stranded ERC-20 recovery, and pending funding UserOperation
       resumption implemented. Sponsored claims now share the validated UserOperation
       publisher with funding, with operation persistence and receipt verification.

5. [x] **Implement on-chain Bitcoin claims and refunds.** Strict HTLC
       reconstruction (version 1, matching current TypeScript/server; reject older versions), UTXO observation through Esplora/Electrum, minimum confirmations,
       network/address checks, timelocks, fee estimation, transaction construction,
       signing, broadcast, and recovery. Test golden scripts/transactions, invalid
       script parameters, insufficient funding, fees/dust, and premature refunds.

6. [x] **Complete Arkade claims and refunds.** BTC→Arkade claim support, VHTLC
       balances and receipt detection, cooperative and timeout refunds without the receiver,
       delegate/cosigner flows, and continuation after interrupted claims. Reuse
       strict VHTLC vectors and validate expected keys, script/address, network,
       amounts, spend state, and preimages before signing or submission.
       Offchain checkpoint: all five Arkade directions, multi-VTXO claims, balances,
       timelocked/cooperative refunds, verified cosigner PSBTs, and persisted
       submit/finalize resumption implemented. Delegated claims, timeout/cooperative
       refunds, mixed/recoverable routing, cosigner verification, and interrupted batch
       reconciliation implemented. Full unilateral wallet exit orchestration is not
       exposed by the reference swap SDK; all six strict script paths remain available.

7. [x] **Implement bridge orchestration.** CCTP domains/tokens and fee estimates,
       burn/approval/permit/user-operation construction, attestation polling,
       inbound funding and recovery, outbound forwarding and recipient persistence;
       USDT0/LayerZero routing, fees, message tracking and recovery. Test fixtures for
       each supported route, pending/failed/complete states, retries, and restart
       resumption. Registry, exact fee arithmetic, burn/OFT call encoding, validated
       attestation and delivery polling, forwarding and explicit reattestation implemented;
       durable wallet/sponsored burn, mint, OFT send, and bounded balance recovery
       implemented with receipt validation and lost-response reconciliation. Atomic
       inbound mint/funding and outbound claim/bridge integration implemented and
       covered by mocked end-to-end/restart tests. EIP-2612 domain/nonce validation,
       typed signing and relay calldata helpers implemented. Sponsored accounts use
       the existing Rust EIP-7702 owner model; external wallets can submit transactions. External providers must remain injectable in offline tests.

8. [x] **Add status subscriptions.** WebSocket connect/subscribe/unsubscribe,
       event decoding, reconnect/resubscribe, bounded queues, explicit shutdown,
       cancellation, and HTTP reconciliation. Test a local WebSocket server with
       disconnects, duplicate/out-of-order updates, malformed payloads, and shutdown.

9. [x] **Implement contract observations and managers.** Typed HTLC references,
       consistent keys, Bitcoin/Arkade/EVM readers, preimage extraction/validation,
       chain-verified funding/spend/expiry observations, reorg/confirmation behavior,
       and cancellation-safe manager lifecycles. Port TypeScript observation vectors.

10. [x] **Implement action tracking and optional automation.** Derive funding,
        claim, cooperative/unilateral refund, bridge recovery, wait, and terminal
        actions from observations; persist/resume tracked swaps; expose action
        subscriptions; reconcile at-risk swaps; optional auto-claim with bounded
        concurrency and failure recovery. Port action-state tests and exercise full
        mocked funding→claim/refund→terminal lifecycles alongside the existing
        creation tests. `AutoClaimer` is the stock claim executor and other
        executors are application adapters; automation is opt-in, reserves
        attempts durably, and never retries ambiguous submissions. Bridge inputs
        must come from verified bridge execution/tracking results.

11. [x] **Finish public API/documentation and integration verification.** Export
        supported entry points; provide examples for each route and recovery path;
        update the coverage table and migration notes; check package contents,
        default and optional feature builds, documentation, and the full test suite.
        Keep a precise list of live network/chain validations that were not executed.
        Added compiled route/tracking/recovery examples, default/all-feature rustdoc
        checks, independent transport-feature checks, and matching CI gates. Package
        contents include examples and this plan. The Arkade 0.11.0 follow-up replaces
        Git pins with published crates; deployment validation remains a follow-up.
        Final review also added pre-submission reservations for EVM funding, refunds,
        and deposit recovery; original-hash reconciliation; refund receipt resumption
        after coordinator deposit deletion; and stricter funding event identity checks.

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

## Final local verification

- Default features: 134 passing tests including two doctests; seven live tests ignored.
- All features: 202 passing tests including two doctests; seven live tests and four
  separately executed local-EVM tests ignored by the ordinary test command.
- Formatting, strict all-target/all-feature Clippy, default/all-feature rustdoc
  with warnings denied, and the websocket+electrum build without EVM: passed.
- All three examples compile with optional features; CI workflow YAML parses.
- `cargo package --list --allow-dirty`: includes source, fixtures, examples and docs.
- `cargo package --locked --allow-dirty`: packaging and verification build passed
  after migrating to the published Arkade 0.11.0 crates. No package was published.
- No production transactions, live bridges, BTCPayServer plugin runs, or .NET
  builds were performed. The README records account-model and trust limitations.

## E2E follow-up

- Four isolated Anvil tests pass against freshly compiled repository contracts:
  signed claims, Permit2 funding and timed refunds, canonical observations after
  a reorg, and restored automatic claim tracking. API snapshots and funding quote
  responses remain fixtures; chain execution is real.
- The full-stack Rust driver now uses the SDK's SQLite store. A separate-process
  regression verifies complete swap records and submission journals survive
  driver restarts. Its tests and strict Clippy pass.
- The Rust SDK CI job explicitly runs the local EVM suite and driver checks.
  Ordinary all-feature tests skip these four EVM tests in addition to the seven
  existing live tests.
- Full-stack EVM→Arkade passed with the swap daemon, Arbitrum fork, local alto
  bundler and Arkade regtest: Rust funded 1 USDC through gasless Permit2, claimed
  1,217 Arkade sats, and the server redeemed the EVM HTLC. Balance assertions
  passed. Polygon was also forked and its contracts deployed during setup.
- Rust wallet boarding passed: 100,000 regtest sats became 100,000 confirmed
  Arkade sats. This is wallet settlement, not a Bitcoin→Arkade server swap.
- Repeated boarding exposed missing single-output spend-status support in the
  local Mempool API. The Rust adapter now falls back to bulk outspends and rejects
  unknown status instead of reselecting spent deposits. Three regression tests
  pass; the boarding scenario now deposits twice into the same wallet.
- All-feature tests after the spend-status fix: 202 passed including doctests;
  strict all-target/all-feature Clippy passed.
- Setup now supports alto host networking and refreshes the daemon's liquidity
  cache after CLI settlement. ShellCheck and shell syntax checks pass.
- At this checkpoint Lightning and bridge scenarios were untested; Lightning
  coverage was added in the follow-up below. No production funds were used.

## Arkade 0.11.0 follow-up

- Replaced the `ark-rs` and `ark-bdk-wallet` Git pins with crates.io 0.11.0 and
  updated the standalone SDK and E2E driver lockfiles.
- Default/all-feature tests and strict Clippy pass with the published crates.
  All four isolated EVM tests pass. The full-stack run passes EVM→Arkade and
  two consecutive boarding deposits with confirmed and total balance assertions.
- Registry package verification now succeeds and runs in the Rust SDK CI job.

## Arkade→EVM E2E follow-up

- The Rust driver now creates Arkade→EVM swaps, funds their VHTLCs through the
  Rust Arkade wallet, and signs gasless EVM claims with the stored per-swap key.
- Full-stack USDC payouts passed on Polygon and Arbitrum, each spending 10,000
  Arkade sats. The runs received 7.451883 USDC and 7.454733 USDC respectively,
  reached `serverredeemed`, and verified client/server balances and the durable
  claim transaction across driver processes.
- The default Rust E2E selection includes both swap directions and repeated
  boarding. Arkade→EVM refund modes still require separate E2E coverage.
- Driver formatting, strict Clippy, process-restart tests, the new scenario's
  Biome checks, and workflow actionlint pass.

## Lightning ↔ EVM E2E follow-up

- Added Rust-backed EVM→Lightning and Lightning→EVM scenarios using the existing
  two-node LND regtest harness and Arbitrum fork/local alto.
- EVM→Lightning created an invoice payout, funded the EVM HTLC through a Rust
  UserOperation, paid the full 20,000-sat invoice and completed server redemption.
  Depositor and server lock-token balance assertions passed.
- Lightning→EVM held a 20,000-sat invoice until the Rust EVM claim revealed the
  preimage, then settled the invoice and completed server redemption. Client USDC
  and server lock-token balance assertions passed.
- Both submission journals survived new driver processes. Driver directions now
  come from typed stored responses instead of inferring BTC rails from tokens.
- Driver formatting, strict all-target Clippy, process-restart tests, TypeScript
  typechecking, scenario Biome checks and runner ShellCheck passed.
- These local LND scenarios are opt-in by name. Lightning refund paths, Lightning
  addresses, hosted Spark and live bridges still need full-stack validation.

## Bitcoin explorer compatibility follow-up

- Real on-chain E2E setup exposed Mempool endpoints missing `/tx/{id}/raw`
  and `/tx/{id}/outspend/{vout}`. The Rust Bitcoin backend now falls back to
  transaction hex and bulk outspends, preserving transaction-ID verification
  and rejecting missing/unknown spend status.
- Four regression tests cover valid hex, wrong transaction IDs, malformed/missing
  transactions, spent/unspent/unknown outputs and native Esplora endpoint use.
- All-feature tests: 206 passed including doctests. Bitcoin-focused tests,
  formatting and strict all-target/all-feature Clippy passed.

- Broadcast follow-up: send transaction hex as `text/plain` for Mempool's body
  parser and verify the returned transaction ID. A real Bitcoin HTLC claim now
  broadcasts successfully. The new regression test checks request content and
  rejection of mismatched IDs. All-feature tests: 207 passed including doctests;
  Bitcoin-focused tests and strict all-target/all-feature Clippy passed.

## On-chain Bitcoin ↔ EVM E2E follow-up

- Added Rust creation/HTLC validation for Bitcoin→EVM and EVM→Bitcoin, a Bitcoin
  claim driver, and complete current server-detail reads through the public SDK.
- Bitcoin→USDC on the Arbitrum fork passed with a 50,000-sat regtest deposit and
  37.459524 USDC payout. The server's confirmed Bitcoin claim spends that HTLC;
  EVM balances and the persisted claim transaction passed assertions.
- USDC on Arbitrum→Bitcoin passed through Rust gasless funding and a Rust-signed
  Bitcoin claim. The confirmed payout was 49,722 sats after a 279-sat claim fee
  (the server funded 50,001 sats for a 50,000-sat quote). Exact input/output/fee
  accounting, destination, EVM balances and server redemption were verified.
- Reopening the driver resumes the same Bitcoin claim without a new spend.
  Funding and both claim journals survive process restarts.
- Both happy paths join the default Rust recipe/manual CI selection. Bitcoin
  refund scenarios and the TS forced-P2WPKH/direct-funding variants remain outside
  these Rust adapters.
- Driver formatting, strict Clippy, process-restart tests, TypeScript checking,
  new-scenario Biome, runner ShellCheck and workflow actionlint passed.

## Full-stack refund E2E follow-up

- Added a Rust refund driver for Bitcoin, Arkade and EVM, plus Bitcoin→Arkade
  creation, preserving SQLite journals across separate SDK processes.
- Added ten sequential refund/cancellation scenarios across all nine routes:
  EVM→Bitcoin/Arkade/Lightning timeout refunds; Bitcoin→EVM/Arkade timeout
  refunds; Arkade→EVM timeout and cooperative refunds; Arkade→Lightning timeout
  refund; Lightning→EVM/Arkade held-payment cancellation and server-leg refund.
- Client refund cases reject premature timeout spends, verify returned balances
  and resume the original refund transaction after restarting the driver.
  Bitcoin refunds additionally verify confirmed HTLC inputs, destinations and
  exact fee accounting; EVM refunds verify mined receipts and the locked amount.
- Lightning-source tests explicitly cancel accepted hold invoices through local
  LND and verify the payer's channel balance returns in full. Automatic Lightning
  HTLC expiry, an SDK cancellation API and hosted providers are outside coverage.
- Clock advancement preserves the canonical Bitcoin chain. The harness waits for
  Mempool to index mined blocks before refund snapshots and for wall time to
  catch up between scenarios. This fixes a real `Bitcoin tip changed during observation` test race without relaxing the SDK's consistency checks.
- HTLC v6 intentionally does not support cooperative EVM refunds. Removed the
  Rust parameter/signing/submission API, its tests and driver mode. Action
  discovery ignores cooperative capability flags for EVM client legs and keeps
  timeout refunds; Arkade cooperative refunds remain supported and tested.
- Refund cases are selectable by name in the runner and manual Rust CI workflow;
  Lightning cases require the opt-in local LND harness. Default Rust CI selection
  retains the happy-path cases.
- All ten cases passed against the local Bitcoin/Arkade/LND stack and Arbitrum
  fork with local alto. Driver restart tests, formatting and strict all-target
  Clippy passed; TypeScript checking, scenario Biome, runner ShellCheck, changed
  workflow actionlint and whitespace checks passed.
- After removing EVM cooperative execution: 208 SDK tests passed, including
  regressions for EVM capability flags and retained Arkade cooperative actions.
  Strict SDK/driver Clippy, driver restart tests, formatting, TypeScript checking
  and scenario Biome passed. EVM→Lightning timeout and Arkade→EVM cooperative
  E2Es both passed again with the updated driver.

## Relayed gasless funding follow-up

- Added `fund_swap_gasless_relay`, the counterpart of TypeScript `fundSwapGasless`
  (`POST /swap/{id}/fund-gasless`), next to the ERC-4337 `fund_swap_gasless`. It
  signs with the swap's own EVM key, sends an unlimited EIP-2612 approval of Permit2
  when there is no allowance, and uses Kernel's ERC-1271 envelope for a delegated
  depositor, as TypeScript on main does.
- Differences from TypeScript: the payload is checked against the stored swap and
  its persisted coordinator, plus a pinned `FundingOptions` when one is given;
  source-token calls are decoded, the relay fee is capped and the permit domain
  and nonce are read from the chain before signing. EVM to Arkade responses carry
  no lock token, so there the payload's is signed unless one is pinned. Each
  attempt is journaled before it is sent and reuses one Permit2 nonce per swap. A
  failure that leaves the journal at `Sending`, `Accepted` or `Uncertain` is
  resolved with `reconcile_gasless_relay_funding` instead of a second request; any
  other failure can be repeated. The funding is recorded only after its receipt
  verifies.
- Unit tests cover payload substitution, the stored lock token and coordinator,
  the journal at request time, lost, refused, reverted and unproven answers,
  timeouts before and after the request, reconciliation and the Kernel envelope. A
  new Anvil test runs the posted signatures through the real Permit2 and
  coordinator.
