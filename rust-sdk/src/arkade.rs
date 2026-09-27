//! Arkade VHTLC claim — offchain spend flow.
//!
//! The lendaswap server, on observing the EVM funding, locks BTC in an
//! Arkade VHTLC paying out to the user (claim path: preimage + receiver
//! sig + server sig). [`Client::claim`] redeems that VHTLC offchain
//! using the per-swap preimage the SDK re-derives from the signer.
//!
//! ## What this module owns
//!
//! `ark-client` requires the consumer to plug in:
//!   - a `Persistence` impl for boarding outputs,
//!   - a [`Blockchain`] impl for esplora-style chain queries,
//!   - an on-chain wallet,
//!   - a [`SwapStorage`](ark_rs::client::swap_storage) for Boltz state.
//!
//! All of that boilerplate is copied near-verbatim from
//! `arkade-os/rust-sdk/ark-client-sample/{main,common}.rs` — the ark-rs
//! README acknowledges the wart and there's no shorter setup today.
//! This module owns the wiring so a downstream consumer's claim site
//! collapses to `client.claim(swap_id, destination).await`.
//!
//! Claims and refunds share validated, resumable multi-output execution.

use crate::client::Client;
use crate::error::Error;
use crate::error::Result;
pub use crate::strict_vhtlc::ARKADE_HTLC_SCRIPT_VERSION_STRICT;
pub use crate::strict_vhtlc::StrictVhtlcOptions;
pub use crate::strict_vhtlc::StrictVhtlcScript;
use ark_bdk_wallet::Wallet;
use ark_rs::client::Blockchain;
use ark_rs::client::BoltzReferralId;
use ark_rs::client::Client as ArkClient;
use ark_rs::client::OfflineClient;
use ark_rs::client::OfflineClientConfig;
use ark_rs::client::SpendStatus;
use ark_rs::client::SqliteContractStore;
use ark_rs::client::TxStatus;
use ark_rs::client::error::Error as ArkError;
use ark_rs::client::swap_storage::InMemorySwapStorage;
use ark_rs::core::ArkAddress;
use ark_rs::core::ExplorerUtxo;
use ark_rs::core::VTXO_CONDITION_KEY;
use ark_rs::core::VtxoList;
use ark_rs::core::send::OffchainTransactions;
use ark_rs::core::send::SendReceiver;
use ark_rs::core::send::VtxoInput;
use ark_rs::core::send::build_offchain_transactions;
use ark_rs::core::server::parse_sequence_number;
use bitcoin::Address;
use bitcoin::Amount;
use bitcoin::Network;
use bitcoin::OutPoint;
use bitcoin::Transaction;
use bitcoin::Txid;
use bitcoin::XOnlyPublicKey;
use bitcoin::bip32::DerivationPath;
use bitcoin::bip32::Xpriv;
use bitcoin::hashes::Hash;
use bitcoin::hashes::ripemd160;
use bitcoin::key::Secp256k1;
use bitcoin::psbt;
use bitcoin::secp256k1;
use bitcoin::secp256k1::Keypair;
use bitcoin::taproot::LeafVersion;
use esplora_client::OutputStatus;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// BIP-85 derivation path for the Arkade identity key. Matches what
/// the TS suite and the e2e binary derive (`scripts/e2e-ts/src/lib/arkade.ts`,
/// `scripts/e2e-rust/src/arkade.rs`), so a wallet recovered from the
/// same mnemonic sees the same VTXOs across SDKs.
const ARKADE_DERIVATION_PATH: &str = "m/83696968'/11811'/0/0";

/// gRPC handshake timeout. Matches the ark-client-sample default.
const ARK_NETWORK_TIMEOUT: Duration = Duration::from_secs(30);

/// Caller-supplied configuration for the Arkade claim flow.
///
/// Mirrors `aa::AaConfig` in shape: URLs + a wallet hook the
/// SDK uses to instantiate the underlying client. The SDK never opens
/// network connections from this struct alone — they happen lazily
/// inside [`crate::Client::claim`].
#[derive(Clone)]
pub struct ArkadeConfig {
    /// gRPC endpoint of the Arkade server (`arkd`).
    pub arkade_server_url: String,
    /// HTTP esplora endpoint backing the on-chain wallet and chain
    /// queries `ark-client` needs.
    pub esplora_url: String,
    /// BIP-39 mnemonic the BIP-85 Arkade identity is derived from.
    /// Must be the SAME mnemonic used to construct the receive address
    /// passed to `create_evm_to_arkade_swap` — otherwise the receiver
    /// keypair in the VHTLC won't match the claim signer.
    pub identity_mnemonic: String,
    /// Network the VHTLC was created on. The SDK pins this to
    /// [`Network::Regtest`] for the e2e harness today; production
    /// wiring will switch on the swap's `network` field.
    pub network: Network,
    /// SQLite DB path for ark-rs contract-manager state. If omitted,
    /// ark-rs uses its default `contracts.db` path relative to the
    /// current working directory.
    pub contract_db_path: Option<PathBuf>,
}

impl std::fmt::Debug for ArkadeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArkadeConfig")
            .field("arkade_server_url", &self.arkade_server_url)
            .field("esplora_url", &self.esplora_url)
            .field("identity_mnemonic", &"<redacted>")
            .field("network", &self.network)
            .field("contract_db_path", &self.contract_db_path)
            .finish()
    }
}

/// Offchain VTXO balance broken into the three buckets `ark-client`
/// distinguishes — what coin selection (`send_offchain`) can use,
/// what's still in-flight, and what can only be recovered via settle.
///
/// Returning all three separately (rather than just `total`) is
/// deliberate: callers that try to send must look at `confirmed_sats`
/// since pre-confirmed/recoverable VTXOs aren't selectable, so a
/// "total"-only view silently bites you with `insufficient funds`
/// even though the headline number looks fine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArkadeBalance {
    /// VTXOs that have been included in a settled batch — the only
    /// bucket coin selection draws from.
    pub confirmed_sats: u64,
    /// VTXOs that exist in an Ark transaction the server has
    /// accepted but that hasn't been confirmed via a settle batch
    /// yet. Become `confirmed_sats` after the next batch finalises.
    pub pre_confirmed_sats: u64,
    /// VTXOs past their settle window — spendable only by joining a
    /// settle batch (no forfeit-tx path). Call `Client::arkade_settle`
    /// to roll them forward.
    pub recoverable_sats: u64,
}

impl ArkadeBalance {
    /// `confirmed + pre_confirmed + recoverable`. Useful for "did funds
    /// arrive at all?" assertions, but **not** the right number for
    /// deciding whether a send will succeed — use `confirmed_sats` for
    /// that.
    pub fn total_sats(&self) -> u64 {
        self.confirmed_sats + self.pre_confirmed_sats + self.recoverable_sats
    }
}

/// Result of a successful Arkade claim.
#[derive(Debug, Clone)]
pub struct ClaimReceipt {
    /// Ark TX ID of the offchain claim transaction (the one that
    /// spends the VHTLC VTXO via the preimage path).
    pub ark_txid: Txid,
    /// Amount swept out of the VHTLC, in satoshis.
    pub claim_amount_sats: u64,
}

/// Connected Arkade client wrapper. Construction performs the gRPC
/// handshake and primes the on-chain wallet; [`Self::offchain_address`]
/// and [`Self::offchain_balance`] are local + cheap once
/// connected, while [`crate::Client::claim`] uses an instance
/// internally for the VHTLC spend.
///
/// Public so callers that need the offchain address BEFORE creating
/// a swap (the e2e suite, or any consumer that derives `receive_to`
/// from a mnemonic) don't have to re-implement the
/// ark-client + ark-bdk-wallet + esplora boilerplate. The inner
/// `client` stays `pub(crate)` so we don't leak the multi-generic
/// `ArkClient<...>` type into the public surface — callers go
/// through the methods on this struct.
pub struct ArkadeWallet {
    pub(crate) client: ArkClient<EsploraBlockchain, Wallet, InMemorySwapStorage>,
}

impl ArkadeWallet {
    /// Build an [`ArkadeWallet`] from `config`: derive the BIP-85
    /// identity, instantiate the on-chain wallet against `esplora_url`,
    /// connect to arkd at `arkade_server_url`.
    pub async fn connect(config: &ArkadeConfig) -> Result<Self> {
        crate::crypto_init::ensure_default_provider_installed();
        let mnemonic = bip39::Mnemonic::from_str(config.identity_mnemonic.trim())
            .map_err(|e| Error::InvalidSigner(format!("Arkade mnemonic: {e}")))?;
        let seed = mnemonic.to_seed("");

        let secp = Secp256k1::new();
        let master = Xpriv::new_master(config.network, &seed)
            .map_err(|e| Error::InvalidSigner(format!("Arkade master xpriv: {e}")))?;
        let path = DerivationPath::from_str(ARKADE_DERIVATION_PATH)
            .map_err(|e| Error::InvalidSigner(format!("Arkade derivation path: {e}")))?;
        let identity_xprv = master
            .derive_priv(&secp, &path)
            .map_err(|e| Error::InvalidSigner(format!("Arkade identity derive: {e}")))?;
        let identity_kp = identity_xprv.to_keypair(&secp);

        let wallet = Wallet::new_from_xpriv(identity_xprv, config.network, &config.esplora_url)
            .map_err(|e| Error::Transport(format!("ark-bdk-wallet: {e}")))?;
        let wallet = Arc::new(wallet);

        let blockchain = Arc::new(EsploraBlockchain::new(&config.esplora_url)?);
        let swap_storage = Arc::new(InMemorySwapStorage::default());

        let contract_store = match &config.contract_db_path {
            Some(path) => SqliteContractStore::new(path),
            None => SqliteContractStore::new_default(),
        }
        .map_err(|e| Error::Transport(format!("ark contract store: {e}")))?;

        let offline = OfflineClient::<_, _, _>::with_keypair(
            OfflineClientConfig {
                ark_server_url: config.arkade_server_url.clone(),
                // Boltz/Lightning URL: we don't exercise it from the claim
                // flow. Empty string keeps the field set without ever
                // dialing it.
                boltz_url: String::new(),
                timeout: ARK_NETWORK_TIMEOUT,
                // Keep connect-time behavior close to the pre-0.10 constructor.
                server_info_ttl: ARK_NETWORK_TIMEOUT,
                // No Boltz interaction from this path.
                boltz_referral_id: BoltzReferralId::Disabled,
                // delegator_pk / historical_delegator_pks: only used for
                // the VTXO delegation flow, which the offchain-claim path
                // explicitly avoids (chunk 5 might add the delegated
                // fallback later if a real swap ever lands in a
                // recoverable VTXO state on us).
                delegator_pk: None,
                historical_delegator_pks: Vec::new(),
            },
            identity_kp,
            blockchain,
            wallet,
            swap_storage,
        )
        .with_contract_store(Box::new(contract_store));

        let client = offline
            .connect()
            .await
            .map_err(|e| Error::Transport(format!("arkd connect: {e}")))?;

        Ok(Self { client })
    }

    /// Derive a fresh offchain Ark address — the bech32m `tark1q…`
    /// string an SDK consumer passes as `receive_to` for an
    /// EVM→Arkade swap (or hands to anyone who wants to send them
    /// BTC offchain). Cheap once [`Self::connect`] has primed the
    /// underlying wallet.
    pub async fn offchain_address(&self) -> Result<String> {
        let (address, _vtxo) = self
            .client
            .get_offchain_address()
            .await
            .map_err(|e| Error::Transport(format!("get_offchain_address: {e}")))?;
        Ok(address.encode())
    }

    /// Offchain VTXO balance broken down into the three buckets
    /// `ark-client` distinguishes — see [`ArkadeBalance`] for details.
    /// Hits the Arkade server's gRPC indexer each call.
    ///
    /// The headline number a caller usually wants is
    /// `confirmed_sats` — that's what coin selection (e.g.
    /// [`Self::send_offchain`]) actually spends. `total_sats()` adds
    /// the in-flight + recoverable buckets so it can over-report what
    /// the wallet can *do right now*.
    pub async fn offchain_balance(&self) -> Result<ArkadeBalance> {
        // Ensure the deterministic default offchain contract is present
        // in ark-rs' contract manager before listing active addresses.
        // Stateless CLI calls may otherwise miss VTXOs created by the TS
        // Arkade SDK for the same identity.
        let _ = self.offchain_address().await?;
        let balance = self
            .client
            .offchain_balance()
            .await
            .map_err(|e| Error::Transport(format!("offchain_balance: {e}")))?;
        Ok(ArkadeBalance {
            confirmed_sats: balance.confirmed().to_sat(),
            pre_confirmed_sats: balance.pre_confirmed().to_sat(),
            recoverable_sats: balance.recoverable().to_sat(),
        })
    }

    /// Roll over all expired VTXOs + boarding outputs into the next
    /// Arkade batch. The wallet ends up with fresh confirmed VTXOs at
    /// the same offchain address, with the exit-window timer reset —
    /// the operation users perform before their VTXOs expire.
    ///
    /// Returns `Ok(None)` if the wallet has nothing to settle (no
    /// boarding inputs and no VTXOs); otherwise the commitment txid
    /// of the batch that absorbed them.
    ///
    /// Uses [`rand::rngs::OsRng`] internally — the underlying ark-rs
    /// settle path needs a CSPRNG for nonce generation.
    ///
    /// Boarding-output discovery in ark-client's
    /// `fetch_commitment_transaction_inputs` uses
    /// `blockchain().find_outpoints(addr)` — a direct esplora call,
    /// not a BDK chain-index lookup — so no `wallet.sync()` is needed
    /// here. The wallet's in-memory DB only needs to hold the
    /// boarding-output *descriptor*, which is persisted by an earlier
    /// `get_boarding_address` call on the same Client-held wallet
    /// instance (the wallet is shared across calls via
    /// `tokio::sync::OnceCell` — see [`crate::Client`]).
    ///
    /// If we later add operations that read from BDK's on-chain UTXO
    /// set (e.g. unilateral exit), those will need to `wallet.sync()`
    /// first.
    pub async fn settle(&self) -> Result<Option<Txid>> {
        let mut rng = rand::rngs::OsRng;
        self.client
            .settle(&mut rng)
            .await
            .map_err(|e| Error::Transport(format!("settle: {e}")))
    }

    /// On-chain Bitcoin "boarding" address for this wallet — send L1
    /// BTC here, then call [`Self::settle`] to promote the boarding
    /// output into a confirmed Arkade VTXO. This is the canonical
    /// "fund my Arkade wallet from regular Bitcoin" path; no
    /// server-mediated swap is involved.
    ///
    /// The address is deterministic per wallet identity (ark-client
    /// always derives the same one for a given signer + server pair),
    /// so it's safe to display once and reuse.
    pub async fn boarding_address(&self) -> Result<String> {
        let address = self
            .client
            .get_boarding_address()
            .await
            .map_err(|e| Error::Transport(format!("get_boarding_address: {e}")))?;
        Ok(address.to_string())
    }

    /// Send `amount_sats` from this wallet to the given Arkade
    /// `destination` address via a single-recipient offchain Ark
    /// transaction. Returns the Ark txid of the offchain spend (NOT a
    /// batch commitment txid — settle is the thing that anchors on L1).
    ///
    /// Used to fund the Arkade VHTLC returned by
    /// [`crate::Client::create_arkade_to_lightning_swap`], but works
    /// for any Arkade destination — including a plain `tark1q…`
    /// address you got from someone else.
    pub async fn send_offchain(&self, destination: &str, amount_sats: u64) -> Result<Txid> {
        let address = ArkAddress::decode(destination).map_err(|e| {
            Error::InvalidSwap(format!("invalid Arkade address `{destination}`: {e}"))
        })?;
        let amount = Amount::from_sat(amount_sats);
        let _ = self.offchain_address().await?;
        self.client
            .send(vec![SendReceiver::bitcoin(address, amount)])
            .await
            .map_err(|e| Error::Transport(format!("send_offchain: {e}")))
    }
}

/// `Blockchain` impl backed by an esplora HTTP server. Copied from the
/// ark-client-sample — the ark-client trait insists on a consumer impl
/// so it can stay backend-agnostic.
pub(crate) struct EsploraBlockchain {
    inner: esplora_client::AsyncClient,
    http: reqwest::Client,
    base_url: String,
}

impl EsploraBlockchain {
    pub(crate) fn new(url: &str) -> Result<Self> {
        let inner = esplora_client::Builder::new(url)
            .build_async()
            .map_err(|e| Error::Transport(format!("esplora client: {e}")))?;
        let http = reqwest::Client::builder()
            .timeout(ARK_NETWORK_TIMEOUT)
            .build()
            .map_err(|e| Error::Transport(format!("esplora client: {e}")))?;
        Ok(Self {
            inner,
            http,
            base_url: url.trim_end_matches('/').into(),
        })
    }

    async fn output_status(
        &self,
        txid: &Txid,
        vout: u32,
    ) -> std::result::Result<OutputStatus, ArkError> {
        if let Some(status) = self
            .inner
            .get_output_status(txid, u64::from(vout))
            .await
            .map_err(ArkError::consumer)?
        {
            return Ok(status);
        }
        // Mempool's Esplora-compatible API may expose only the bulk endpoint.
        // A missing single-output response does not establish that it is unspent.
        let statuses: Vec<OutputStatus> = self
            .http
            .get(format!("{}/tx/{txid}/outspends", self.base_url))
            .send()
            .await
            .map_err(ArkError::consumer)?
            .error_for_status()
            .map_err(ArkError::consumer)?
            .json()
            .await
            .map_err(ArkError::consumer)?;
        statuses
            .into_iter()
            .nth(vout as usize)
            .ok_or_else(|| ArkError::consumer("Esplora output spend status unknown"))
    }
}

impl Blockchain for EsploraBlockchain {
    async fn find_outpoints(
        &self,
        address: &Address,
    ) -> std::result::Result<Vec<ExplorerUtxo>, ArkError> {
        let current_block_height = self.inner.get_height().await.map_err(ArkError::consumer)?;
        let script_pubkey = address.script_pubkey();
        let txs = crate::bitcoin::scripthash_history(
            &self.inner,
            &self.http,
            &self.base_url,
            &script_pubkey,
        )
        .await
        .map_err(ArkError::consumer)?;

        let outputs = txs
            .into_iter()
            .flat_map(|tx| {
                let txid = tx.txid;
                tx.vout
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.scriptpubkey == script_pubkey)
                    .map(|(i, v)| {
                        let confirmations = match tx.status.block_height {
                            Some(h) => match current_block_height.checked_sub(h) {
                                Some(d) => d + 1,
                                None => 0,
                            },
                            None => 0,
                        };
                        ExplorerUtxo {
                            outpoint: OutPoint {
                                txid,
                                vout: i as u32,
                            },
                            amount: Amount::from_sat(v.value),
                            confirmation_blocktime: tx.status.block_time,
                            confirmations: confirmations as u64,
                            is_spent: false,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let mut utxos = Vec::new();
        for output in outputs.iter() {
            let outpoint = output.outpoint;
            let status = self.output_status(&outpoint.txid, outpoint.vout).await?;
            match status {
                OutputStatus { spent: false, .. } => utxos.push(*output),
                OutputStatus { spent: true, .. } => utxos.push(ExplorerUtxo {
                    is_spent: true,
                    ..*output
                }),
            }
        }
        Ok(utxos)
    }

    async fn find_tx(&self, txid: &Txid) -> std::result::Result<Option<Transaction>, ArkError> {
        self.inner.get_tx(txid).await.map_err(ArkError::consumer)
    }

    async fn get_tx_status(&self, txid: &Txid) -> std::result::Result<TxStatus, ArkError> {
        let info = self
            .inner
            .get_tx_info(txid)
            .await
            .map_err(ArkError::consumer)?;
        Ok(TxStatus {
            confirmed_at: info.and_then(|s| s.status.block_time.map(|t| t as i64)),
        })
    }

    async fn get_output_status(
        &self,
        txid: &Txid,
        vout: u32,
    ) -> std::result::Result<SpendStatus, ArkError> {
        let status = self.output_status(txid, vout).await?;
        if status.spent && status.txid.is_none() {
            return Err(ArkError::consumer(
                "Esplora spent output missing spending transaction",
            ));
        }
        Ok(SpendStatus {
            spend_txid: if status.spent { status.txid } else { None },
        })
    }

    async fn broadcast(&self, tx: &Transaction) -> std::result::Result<(), ArkError> {
        self.inner.broadcast(tx).await.map_err(ArkError::consumer)
    }

    async fn get_fee_rate(&self) -> std::result::Result<f64, ArkError> {
        // Regtest doesn't have a meaningful fee market. The offchain
        // claim path doesn't broadcast an on-chain tx anyway — this is
        // only consulted for unilateral-exit code paths we don't hit.
        Ok(1.0)
    }

    async fn broadcast_package(&self, _txs: &[&Transaction]) -> std::result::Result<(), ArkError> {
        // Not used by the offchain claim flow. Add a real impl if/when
        // we wire unilateral exit.
        Err(ArkError::consumer(
            "broadcast_package not implemented".to_string(),
        ))
    }
}

// ── claim entry point ──────────────────────────────────────────────────

mod delegate;
mod execution;
pub use delegate::ArkadeDelegateState;
pub use delegate::ArkadeSettlement;
pub use execution::ArkadeBackend;
pub use execution::ArkadeFuture;
pub use execution::ArkadeHtlc;
pub use execution::ArkadePending;
pub use execution::ArkadeSpend;
pub use execution::ArkadeSpendOptions;
pub use execution::VhtlcBalance;

impl Client {
    /// Claim an EVM, Lightning, or Bitcoin to Arkade swap. Progress is persisted
    /// before submission and finalization so the same call can resume after restart.
    pub async fn claim(&self, swap_id: &str, destination: &str) -> Result<ClaimReceipt> {
        if self.get_stored_swap(swap_id)?.is_none() {
            self.refresh_swap(swap_id).await?;
        }
        self.arkade_htlc(swap_id)?;
        let backend = self.arkade_wallet().await?;
        self.spend_arkade(
            swap_id,
            backend,
            None,
            ArkadeSpend::Claim,
            ArkadeSpendOptions {
                destination: destination.into(),
                timeout: Duration::from_secs(120),
                minimum_remaining_seconds: 300,
            },
        )
        .await
    }

    /// Refund after the VHTLC timelock, verified against the configured Bitcoin chain.
    pub async fn refund_arkade(&self, swap_id: &str, destination: &str) -> Result<ClaimReceipt> {
        let config = self.arkade_config()?;
        let chain = crate::bitcoin::EsploraBackend::new(&config.esplora_url, config.network)?;
        let backend = self.arkade_wallet().await?;
        self.spend_arkade(
            swap_id,
            backend,
            Some(&chain),
            ArkadeSpend::Refund,
            ArkadeSpendOptions {
                destination: destination.into(),
                timeout: Duration::from_secs(120),
                minimum_remaining_seconds: 0,
            },
        )
        .await
    }

    pub async fn collab_refund_arkade(
        &self,
        swap_id: &str,
        destination: &str,
    ) -> Result<ClaimReceipt> {
        let backend = self.arkade_wallet().await?;
        self.spend_arkade(
            swap_id,
            backend,
            None,
            ArkadeSpend::CooperativeRefund,
            ArkadeSpendOptions {
                destination: destination.into(),
                timeout: Duration::from_secs(120),
                minimum_remaining_seconds: 0,
            },
        )
        .await
    }
}

#[cfg(test)]
mod esplora_tests {
    use super::*;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;

    #[tokio::test]
    async fn bulk_outspends_preserve_spent_and_unspent_outputs() {
        let server = MockServer::start().await;
        let txid = Txid::from_str(&"11".repeat(32)).unwrap();
        let spending_txid = Txid::from_str(&"22".repeat(32)).unwrap();
        Mock::given(path(format!("/tx/{txid}/outspends")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"spent": false},
                {"spent": true, "txid": spending_txid, "vin": 0}
            ])))
            .expect(2)
            .mount(&server)
            .await;
        let chain = EsploraBlockchain::new(&server.uri()).unwrap();
        assert!(!chain.output_status(&txid, 0).await.unwrap().spent);
        assert_eq!(
            Blockchain::get_output_status(&chain, &txid, 1)
                .await
                .unwrap()
                .spend_txid,
            Some(spending_txid)
        );
    }

    #[tokio::test]
    async fn absent_output_status_never_means_unspent() {
        for status in [404, 200] {
            let server = MockServer::start().await;
            let txid = Txid::from_str(&"11".repeat(32)).unwrap();
            Mock::given(path(format!("/tx/{txid}/outspends")))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!([])))
                .mount(&server)
                .await;
            let chain = EsploraBlockchain::new(&server.uri()).unwrap();
            assert!(chain.output_status(&txid, 0).await.is_err());
            assert!(
                Blockchain::get_output_status(&chain, &txid, 0)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn single_output_endpoint_is_used_when_available() {
        let server = MockServer::start().await;
        let txid = Txid::from_str(&"11".repeat(32)).unwrap();
        Mock::given(path(format!("/tx/{txid}/outspend/0")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"spent": false})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path(format!("/tx/{txid}/outspends")))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let chain = EsploraBlockchain::new(&server.uri()).unwrap();
        assert!(
            Blockchain::get_output_status(&chain, &txid, 0)
                .await
                .unwrap()
                .spend_txid
                .is_none()
        );
    }
}
