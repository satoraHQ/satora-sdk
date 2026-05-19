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
//!   - a [`Persistence`] impl for boarding outputs,
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
//! Gated behind the `arkade-claim` feature — the deps it pulls in
//! (`ark-rs` with gRPC, `ark-bdk-wallet`, `esplora-client`) cost real
//! compile time the EVM-only SDK doesn't want to pay for.
//!
//! Today this is scaffolding only; the actual claim flow lands in
//! follow-up commits as we work through the [`Client::claim`] body.

use crate::error::Error;
use crate::error::Result;
use ark_bdk_wallet::Wallet;
use ark_rs::client::Blockchain;
use ark_rs::client::Client as ArkClient;
use ark_rs::client::OfflineClient;
use ark_rs::client::SpendStatus;
use ark_rs::client::StaticKeyProvider;
use ark_rs::client::TxStatus;
use ark_rs::client::error::Error as ArkError;
use ark_rs::client::swap_storage::InMemorySwapStorage;
use ark_rs::client::wallet::Persistence;
use ark_rs::core::BoardingOutput;
use ark_rs::core::ExplorerUtxo;
use bitcoin::Address;
use bitcoin::Amount;
use bitcoin::Network;
use bitcoin::OutPoint;
use bitcoin::Transaction;
use bitcoin::Txid;
use bitcoin::XOnlyPublicKey;
use bitcoin::bip32::DerivationPath;
use bitcoin::bip32::Xpriv;
use bitcoin::key::Secp256k1;
use bitcoin::secp256k1::SecretKey;
use esplora_client::OutputStatus;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::RwLock;
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
/// Mirrors [`crate::aa::AaConfig`] in shape: URLs + a wallet hook the
/// SDK uses to instantiate the underlying client. The SDK never opens
/// network connections from this struct alone — they happen lazily
/// inside [`crate::Client::claim`].
#[derive(Debug, Clone)]
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
/// handshake and primes the on-chain wallet; [`Self::claim_vhtlc_offchain`]
/// (added in a follow-up commit) does the actual VHTLC spend.
///
/// Held internally by [`crate::Client::claim`] — not exposed publicly
/// because the claim entry point on `Client` is what callers should
/// use. The struct stays `pub(crate)` for the same reason `aa::bundler`
/// keeps `BundlerClient` internal.
pub(crate) struct ArkadeWallet {
    pub(crate) client:
        ArkClient<EsploraBlockchain, Wallet<InMemoryDb>, InMemorySwapStorage, StaticKeyProvider>,
}

impl ArkadeWallet {
    /// Build an [`ArkadeWallet`] from `config`: derive the BIP-85
    /// identity, instantiate the on-chain wallet against `esplora_url`,
    /// connect to arkd at `arkade_server_url`.
    pub(crate) async fn connect(config: &ArkadeConfig) -> Result<Self> {
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

        let db = InMemoryDb::default();
        let wallet = Wallet::new(identity_kp, secp, config.network, &config.esplora_url, db)
            .map_err(|e| Error::Transport(format!("ark-bdk-wallet: {e}")))?;
        let wallet = Arc::new(wallet);

        let blockchain = Arc::new(EsploraBlockchain::new(&config.esplora_url)?);
        let swap_storage = Arc::new(InMemorySwapStorage::default());

        let offline = OfflineClient::<_, _, _, StaticKeyProvider>::new_with_keypair(
            "lendaswap-sdk".to_string(),
            identity_kp,
            blockchain,
            wallet,
            config.arkade_server_url.clone(),
            swap_storage,
            // Boltz/Lightning URL: we don't exercise it from the claim
            // flow. Empty string keeps the field set without ever
            // dialing it.
            String::new(),
            ARK_NETWORK_TIMEOUT,
            // delegator_pk / historical_delegator_pks: only used for
            // the VTXO delegation flow, which the offchain-claim path
            // explicitly avoids (chunk 5 might add the delegated
            // fallback later if a real swap ever lands in a
            // recoverable VTXO state on us).
            None,
            Vec::new(),
        );

        let client = offline
            .connect()
            .await
            .map_err(|e| Error::Transport(format!("arkd connect: {e}")))?;

        Ok(Self { client })
    }
}

/// Boarding-output persistence backing `ark-client`. In-memory is fine
/// for one-shot claim flows; the SDK doesn't survive a process restart
/// today, and the data we'd persist (boarding-output → secret-key) is
/// re-derivable from the mnemonic on the next call.
#[derive(Default)]
pub(crate) struct InMemoryDb {
    boarding_outputs: RwLock<HashMap<BoardingOutput, SecretKey>>,
}

impl Persistence for InMemoryDb {
    fn save_boarding_output(
        &self,
        sk: SecretKey,
        boarding_output: BoardingOutput,
    ) -> std::result::Result<(), ArkError> {
        self.boarding_outputs
            .write()
            .map_err(|e| ArkError::consumer(format!("write lock: {e}")))?
            .insert(boarding_output, sk);
        Ok(())
    }

    fn load_boarding_outputs(&self) -> std::result::Result<Vec<BoardingOutput>, ArkError> {
        Ok(self
            .boarding_outputs
            .read()
            .map_err(|e| ArkError::consumer(format!("read lock: {e}")))?
            .keys()
            .cloned()
            .collect())
    }

    fn sk_for_pk(&self, pk: &XOnlyPublicKey) -> std::result::Result<SecretKey, ArkError> {
        self.boarding_outputs
            .read()
            .map_err(|e| ArkError::consumer(format!("read lock: {e}")))?
            .iter()
            .find_map(|(b, sk)| if b.owner_pk() == *pk { Some(*sk) } else { None })
            .ok_or_else(|| ArkError::consumer(format!("no sk for pk {pk}")))
    }
}

/// `Blockchain` impl backed by an esplora HTTP server. Copied from the
/// ark-client-sample — the ark-client trait insists on a consumer impl
/// so it can stay backend-agnostic.
pub(crate) struct EsploraBlockchain {
    inner: esplora_client::AsyncClient,
}

impl EsploraBlockchain {
    pub(crate) fn new(url: &str) -> Result<Self> {
        let inner = esplora_client::Builder::new(url)
            .build_async()
            .map_err(|e| Error::Transport(format!("esplora client: {e}")))?;
        Ok(Self { inner })
    }
}

impl Blockchain for EsploraBlockchain {
    async fn find_outpoints(
        &self,
        address: &Address,
    ) -> std::result::Result<Vec<ExplorerUtxo>, ArkError> {
        let current_block_height = self.inner.get_height().await.map_err(ArkError::consumer)?;
        let script_pubkey = address.script_pubkey();
        let txs = self
            .inner
            .scripthash_txs(&script_pubkey, None)
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
            let status = self
                .inner
                .get_output_status(&outpoint.txid, outpoint.vout as u64)
                .await
                .map_err(ArkError::consumer)?;
            match status {
                Some(OutputStatus { spent: false, .. }) | None => utxos.push(*output),
                Some(OutputStatus { spent: true, .. }) => utxos.push(ExplorerUtxo {
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
        let status = self
            .inner
            .get_output_status(txid, vout as u64)
            .await
            .map_err(ArkError::consumer)?;
        Ok(SpendStatus {
            spend_txid: status.as_ref().and_then(|s| s.txid),
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
