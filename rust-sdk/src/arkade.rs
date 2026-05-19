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

use crate::client::Client;
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
use ark_rs::core::ArkAddress;
use ark_rs::core::BoardingOutput;
use ark_rs::core::ExplorerUtxo;
use ark_rs::core::VTXO_CONDITION_KEY;
use ark_rs::core::VtxoList;
use ark_rs::core::send::OffchainTransactions;
use ark_rs::core::send::SendReceiver;
use ark_rs::core::send::VtxoInput;
use ark_rs::core::send::build_offchain_transactions;
use ark_rs::core::send::sign_ark_transaction;
use ark_rs::core::server::parse_sequence_number;
use ark_rs::core::vhtlc::VhtlcOptions;
use ark_rs::core::vhtlc::VhtlcScript;
use bitcoin::Address;
use bitcoin::Amount;
use bitcoin::Network;
use bitcoin::OutPoint;
use bitcoin::Transaction;
use bitcoin::Txid;
use bitcoin::VarInt;
use bitcoin::XOnlyPublicKey;
use bitcoin::bip32::DerivationPath;
use bitcoin::bip32::Xpriv;
use bitcoin::consensus::Encodable;
use bitcoin::hashes::Hash;
use bitcoin::hashes::ripemd160;
use bitcoin::hashes::sha256;
use bitcoin::key::Secp256k1;
use bitcoin::psbt;
use bitcoin::secp256k1;
use bitcoin::secp256k1::Keypair;
use bitcoin::secp256k1::SecretKey;
use bitcoin::secp256k1::schnorr;
use bitcoin::taproot::LeafVersion;
use esplora_client::OutputStatus;
use std::collections::HashMap;
use std::io::Write;
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

// ── claim entry point ──────────────────────────────────────────────────

impl Client {
    /// Redeem the Arkade VHTLC for an EVM→Arkade swap that has reached
    /// (or passed) [`crate::types::SwapStatus::ServerFunded`].
    ///
    /// Re-derives the preimage from the signer + the swap's `key_index`
    /// in [`crate::SwapStorage`], rebuilds the VHTLC script from the
    /// backend's view of the swap, fetches the VTXO at the VHTLC
    /// address, and offchain-spends it to `destination` via the claim
    /// script (preimage + receiver sig + server sig).
    ///
    /// The body lands in follow-up commits; this commit just validates
    /// the prerequisites (storage lookup, key derivation, response
    /// fetch, VHTLC script reconstruction + address sanity check) so a
    /// caller hitting a config mismatch fails fast with a clear error
    /// rather than midway through the gRPC handshake.
    #[tracing::instrument(name = "claim", skip_all, fields(%swap_id))]
    pub async fn claim(
        &self,
        swap_id: &str,
        destination: &str,
        config: ArkadeConfig,
    ) -> Result<ClaimReceipt> {
        // 1. Re-derive the per-swap secret material from the signer. The preimage isn't persisted
        //    by the SDK; it's deterministic from (signer master seed, key_index), which IS
        //    persisted.
        let signer = self.signer.as_ref().ok_or_else(|| {
            Error::InvalidSigner(
                "Client constructed without a signer — use Client::builder() with .mnemonic / .xprv"
                    .to_string(),
            )
        })?;
        let key_index = self.storage.get_swap_key_index(swap_id)?.ok_or_else(|| {
            Error::InvalidSwap(format!(
                "no key_index in storage for swap `{swap_id}` — has create_swap been called on this Client?",
            ))
        })?;
        let swap_params = signer.derive_swap_params(key_index)?;

        // 2. Fetch the backend's current view of the swap — `Swap` drops the VHTLC pubkeys +
        //    locktimes we need for the script.
        let resp = self.fetch_swap_response(swap_id).await?;

        // Sanity: SDK-derived hash_lock must match the backend's. A
        // mismatch means a different mnemonic / wrong key_index — the
        // claim would silently fail later when Permit2 verifies our
        // sig against an unexpected pubkey.
        let backend_hash_lock = parse_hash_lock(&resp.hash_lock)?;
        if backend_hash_lock != swap_params.hash_lock {
            return Err(Error::InvalidSwap(format!(
                "hash_lock mismatch: backend has {} but signer/key_index derived a different preimage",
                resp.hash_lock,
            )));
        }

        // 3. Rebuild the VHTLC script from the response and the user-derived receiver pubkey. The
        //    sender / server pubkeys come from the backend; the receiver pubkey we trust comes from
        //    our local signer (matches `receiver_pk` on the response by construction, but it's
        //    safer to use the local one for signing later).
        let vhtlc = build_vhtlc_script(&resp, &swap_params, config.network)?;

        // Sanity: computed VHTLC address must match what the backend
        // told us. If not, we're rebuilding the wrong script — bail
        // before spending anything.
        let computed_address = vhtlc.address().encode();
        if computed_address != resp.btc_vhtlc_address {
            return Err(Error::InvalidSwap(format!(
                "VHTLC address mismatch: computed `{computed_address}`, backend says `{}`",
                resp.btc_vhtlc_address,
            )));
        }

        // 4. Wire up the Arkade client. The connect blocks on gRPC, which is why everything that
        //    can fail without network is above this line.
        let arkade = ArkadeWallet::connect(&config).await?;

        // 5. Find the VHTLC's VTXO. The VHTLC produces exactly one output, but the indexer can take
        //    a moment to surface it even after the backend reaches ServerFunded — callers that race
        //    the indexer should poll on top of this rather than retry inside the claim path.
        let vhtlc_ark_address = vhtlc.address();
        let virtual_tx_outpoints = arkade
            .client
            .get_virtual_tx_outpoints(std::iter::once(vhtlc_ark_address))
            .await
            .map_err(|e| Error::Transport(format!("get_virtual_tx_outpoints: {e}")))?;

        let vtxo_list = VtxoList::new(arkade.client.server_info.dust, virtual_tx_outpoints);
        let vhtlc_outpoint = vtxo_list
            .all_unspent()
            .next()
            .ok_or_else(|| {
                Error::InvalidSwap(format!(
                    "no unspent VTXO at VHTLC address {} — funding not yet visible to the Arkade indexer",
                    resp.btc_vhtlc_address,
                ))
            })?
            .clone();
        let claim_amount = vhtlc_outpoint.amount;
        tracing::info!(
            amount_sats = claim_amount.to_sat(),
            outpoint = %vhtlc_outpoint.outpoint,
            "claim: located VHTLC VTXO",
        );

        // 6. Construct the VtxoInput spending via the VHTLC's claim script (preimage + receiver sig
        //    + server sig). The control block lets the spending tx prove the script is in the
        //    Taproot tree without revealing the other leaves.
        let destination_address = ArkAddress::decode(destination)
            .map_err(|e| Error::InvalidSwap(format!("destination ArkAddress::decode: {e}")))?;

        let spend_info = vhtlc.taproot_spend_info();
        let claim_script = vhtlc.claim_script();
        let script_ver = (claim_script.clone(), LeafVersion::TapScript);
        let control_block = spend_info.control_block(&script_ver).ok_or_else(|| {
            Error::InvalidSwap(
                "control block not found for claim script — VhtlcScript out of sync".to_string(),
            )
        })?;

        // `tapscripts(self)` consumes the VHTLC by value, so capture
        // any other fields we still need (script_pubkey) first.
        let script_pubkey = vhtlc.script_pubkey();
        let tapscripts = vhtlc.tapscripts();
        let vhtlc_input = VtxoInput::new(
            claim_script,
            None,
            control_block,
            tapscripts,
            script_pubkey,
            claim_amount,
            vhtlc_outpoint.outpoint,
            vhtlc_outpoint.assets.clone(),
        );

        let outputs = vec![SendReceiver::bitcoin(destination_address, claim_amount)];
        // We're draining the VHTLC entirely; reuse the destination as the change address (no
        // change will actually be produced).
        let OffchainTransactions {
            mut ark_tx,
            checkpoint_txs,
        } = build_offchain_transactions(
            &outputs,
            &destination_address,
            std::slice::from_ref(&vhtlc_input),
            &arkade.client.server_info,
        )
        .map_err(|e| Error::Decode(format!("build_offchain_transactions: {e}")))?;

        // 7. Sign the Ark TX with the user's per-swap secp256k1 key AND embed the preimage as a
        //    PSBT field under the VHTLC's condition key (type=222) — the Ark server reads it on
        //    submit to satisfy the OP_HASH160 branch of the claim script. Layout per ark-core's
        //    witness format: 0x01 || varint(preimage_len) || preimage
        let secret_key = secp256k1::SecretKey::from_slice(&swap_params.secret)
            .map_err(|e| Error::InvalidSigner(format!("secp256k1 SecretKey::from_slice: {e}")))?;
        let secp = Secp256k1::new();
        let claimer_kp = Keypair::from_secret_key(&secp, &secret_key);
        let preimage_bytes = swap_params.preimage;
        let sign_fn = |input: &mut psbt::Input,
                       msg: secp256k1::Message|
         -> std::result::Result<
            Vec<(schnorr::Signature, XOnlyPublicKey)>,
            ark_rs::core::Error,
        > {
            // Encode the preimage witness: 0x01 (count) || varint(len) || preimage.
            let mut bytes = vec![1u8];
            VarInt::from(preimage_bytes.len() as u64)
                .consensus_encode(&mut bytes)
                .expect("varint encode never fails on Vec");
            bytes
                .write_all(&preimage_bytes)
                .expect("write to Vec never fails");
            input.unknown.insert(
                psbt::raw::Key {
                    type_value: 222,
                    key: VTXO_CONDITION_KEY.to_vec(),
                },
                bytes,
            );

            let sig = Secp256k1::new().sign_schnorr_no_aux_rand(&msg, &claimer_kp);
            let pk = claimer_kp.x_only_public_key().0;
            Ok(vec![(sig, pk)])
        };

        sign_ark_transaction(sign_fn, &mut ark_tx, 0)
            .map_err(|e| Error::Decode(format!("sign_ark_transaction: {e}")))?;
        let ark_txid = ark_tx.unsigned_tx.compute_txid();
        tracing::info!(%ark_txid, "claim: ark TX signed");

        // 8: submit + finalize — implemented in the next commit.
        let _ = checkpoint_txs;
        Err(Error::InvalidSwap(
            "Client::claim: submit + finalize not implemented yet (next commit)".to_string(),
        ))
    }
}

// ── helpers ────────────────────────────────────────────────────────────

/// Parse the backend's `hash_lock` hex string into a `[u8; 32]` for
/// equality comparison with the SDK-derived value.
fn parse_hash_lock(hex_str: &str) -> Result<[u8; 32]> {
    let trimmed = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(trimmed)
        .map_err(|e| Error::Decode(format!("hash_lock hex: {e} (value: {hex_str})")))?;
    bytes.try_into().map_err(|v: Vec<u8>| {
        Error::Decode(format!(
            "hash_lock length: expected 32 bytes, got {}",
            v.len()
        ))
    })
}

/// Parse a 32-byte x-only public key from a hex string.
///
/// Accepts both the bare 32-byte form (64 hex chars) and the SEC-1
/// compressed form (66 hex chars with a `02`/`03` parity prefix) —
/// the backend uses the latter for VHTLC pubkeys.
fn parse_xonly_pubkey(hex_str: &str, field: &str) -> Result<XOnlyPublicKey> {
    let trimmed = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(trimmed)
        .map_err(|e| Error::Decode(format!("{field} hex: {e} (value: {hex_str})")))?;
    let xonly_bytes: &[u8] = match bytes.len() {
        32 => &bytes,
        33 => &bytes[1..],
        n => {
            return Err(Error::Decode(format!(
                "{field}: expected 32 or 33 bytes, got {n}"
            )));
        }
    };
    XOnlyPublicKey::from_slice(xonly_bytes)
        .map_err(|e| Error::Decode(format!("{field} XOnlyPublicKey::from_slice: {e}")))
}

/// Build the VHTLC script from the backend's swap response.
///
/// The VHTLC's three roles map onto the EVM-→-Arkade swap as:
///   - `sender` = lendaswap server (it's the one that locked BTC)
///   - `receiver` = the user (whose key the SDK signs claims with)
///   - `server` = Arkade server (third-party cosigner)
///
/// `preimage_hash` is `HASH160(preimage) = RIPEMD160(SHA256(preimage))`
/// — distinct from the EVM HTLC's `hash_lock = SHA256(preimage)`.
fn build_vhtlc_script(
    resp: &crate::types::EvmToArkadeSwapResponse,
    swap_params: &crate::signer::SwapParams,
    network: Network,
) -> Result<VhtlcScript> {
    let sender = parse_xonly_pubkey(&resp.sender_pk, "sender_pk")?;
    let receiver = parse_xonly_pubkey(&resp.receiver_pk, "receiver_pk")?;
    let server = parse_xonly_pubkey(&resp.arkade_server_pk, "arkade_server_pk")?;

    // Backend's `receiver_pk` should match what we derive from the
    // signer — if not, this client can't sign the claim regardless of
    // what other addresses agree on. The SDK's `derive_swap_params`
    // returns a SEC-1 compressed (33 byte) pubkey; strip the parity
    // byte to compare against the x-only form the VHTLC uses.
    let derived_receiver = XOnlyPublicKey::from_slice(&swap_params.public_key[1..])
        .map_err(|e| Error::InvalidSigner(format!("derived receiver_pk: {e}")))?;
    if derived_receiver != receiver {
        return Err(Error::InvalidSwap(format!(
            "receiver_pk mismatch: backend `{}` vs locally-derived",
            resp.receiver_pk,
        )));
    }

    let preimage_hash = ripemd160::Hash::hash(
        sha256::Hash::hash(&swap_params.preimage)
            .as_byte_array()
            .as_slice(),
    );

    let refund_locktime = u32::try_from(resp.vhtlc_refund_locktime).map_err(|_| {
        Error::Decode(format!(
            "vhtlc_refund_locktime {} doesn't fit in u32",
            resp.vhtlc_refund_locktime,
        ))
    })?;
    let unilateral_claim_delay = parse_sequence_number(resp.unilateral_claim_delay as i64)
        .map_err(|e| Error::Decode(format!("unilateral_claim_delay: {e}")))?;
    let unilateral_refund_delay = parse_sequence_number(resp.unilateral_refund_delay as i64)
        .map_err(|e| Error::Decode(format!("unilateral_refund_delay: {e}")))?;
    let unilateral_refund_without_receiver_delay =
        parse_sequence_number(resp.unilateral_refund_without_receiver_delay as i64)
            .map_err(|e| Error::Decode(format!("unilateral_refund_without_receiver_delay: {e}")))?;

    VhtlcScript::new(
        VhtlcOptions {
            sender,
            receiver,
            server,
            preimage_hash,
            refund_locktime,
            unilateral_claim_delay,
            unilateral_refund_delay,
            unilateral_refund_without_receiver_delay,
        },
        network,
    )
    .map_err(|e| Error::Decode(format!("VhtlcScript::new: {e}")))
}
