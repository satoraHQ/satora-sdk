//! Chain observations and bounded contract managers. API status is never an
//! input to this layer. Failed or stale reads cannot authorize an action.
use crate::Error;
use crate::Result;
use bitcoin::Network;
use bitcoin::hashes::Hash;
use bitcoin::hashes::hash160;
use bitcoin::hashes::sha256;
use std::future::Future;
use std::pin::Pin;

mod references;
pub use references::ReaderSet;
pub use references::ReferenceOptions;
pub use references::SwapContracts;
mod manager;
pub use manager::ContractManager;
pub use manager::ManagerOptions;
pub use manager::ObservationEvent;
pub use manager::ObservationMonitor;
mod bitcoin_reader;
pub use bitcoin_reader::BitcoinReader;
mod arkade_reader;
pub use arkade_reader::ArkadeReader;
#[cfg(feature = "gasless")]
mod evm_reader;
#[cfg(feature = "gasless")]
pub use evm_reader::EvmRpcReader;

pub type ObservationFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// A client's Bitcoin confirmation depth, shared by its clones and by the
/// managers of trackers built from it, so a change reaches the next read.
#[derive(Clone)]
pub(crate) struct BitcoinDepth(std::sync::Arc<tokio::sync::watch::Sender<Option<u32>>>);
impl BitcoinDepth {
    pub(crate) fn new(confirmations: Option<u32>) -> Self {
        Self(std::sync::Arc::new(
            tokio::sync::watch::channel(confirmations).0,
        ))
    }
    pub(crate) fn get(&self) -> Option<u32> {
        *self.0.borrow()
    }
    pub(crate) fn set(&self, confirmations: Option<u32>) {
        self.0.send_replace(confirmations);
    }
    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<Option<u32>> {
        self.0.subscribe()
    }
    fn same(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "ledger", rename_all = "snake_case")]
pub enum ContractRef {
    Bitcoin {
        htlc: crate::bitcoin::BitcoinHtlc,
        expected_sats: u64,
        /// Confirmations the funding needs before it reads as confirmed; a spend
        /// counts at any depth. None follows `Client::bitcoin_min_confirmations`,
        /// resolved on every read by a manager that follows that client (see
        /// `ContractManager::follow_bitcoin_min_confirmations`), and reads as 0
        /// anywhere else.
        confirmations: Option<u32>,
    },
    Arkade {
        options: crate::arkade::StrictVhtlcOptions,
        network: Network,
        expected_sats: u64,
    },
    #[cfg(feature = "gasless")]
    Evm {
        htlc: crate::evm::htlc::EvmHtlc,
        from_block: u64,
        confirmations: u32,
    },
}
impl ContractRef {
    /// Includes the entire EVM swap tuple, not just its shared hash lock.
    pub fn key(&self) -> Result<String> {
        Ok(match self {
            Self::Bitcoin { htlc, .. } => format!("bitcoin:{}:{}", htlc.network, htlc.address()?),
            Self::Arkade {
                options, network, ..
            } => format!(
                "arkade:{network}:{}",
                crate::arkade::StrictVhtlcScript::new(options.clone(), *network)?
                    .script_pubkey()
                    .to_hex_string()
            ),
            #[cfg(feature = "gasless")]
            Self::Evm { htlc, .. } => {
                format!("evm:{}:{}:{}", htlc.chain_id, htlc.address, htlc.key())
            }
        })
    }
    pub fn timelock(&self) -> Timelock {
        match self {
            Self::Bitcoin { htlc, .. } => Timelock::bitcoin(htlc.refund_locktime),
            Self::Arkade { options, .. } => Timelock::bitcoin(options.refund_locktime),
            #[cfg(feature = "gasless")]
            Self::Evm { htlc, .. } => Timelock::Timestamp(htlc.timelock),
        }
    }
    pub fn hash_lock(&self) -> HashLock {
        match self {
            Self::Bitcoin { htlc, .. } => HashLock::Hash160(htlc.hash_lock),
            Self::Arkade { options, .. } => {
                HashLock::Hash160(options.preimage_hash.to_byte_array())
            }
            #[cfg(feature = "gasless")]
            Self::Evm { htlc, .. } => HashLock::Sha256(htlc.hash_lock.0),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timelock {
    Height(u64),
    Timestamp(u64),
    MedianTime(u64),
}
impl Timelock {
    pub fn bitcoin(locktime: u32) -> Self {
        if locktime < 500_000_000 {
            Self::Height(u64::from(locktime))
        } else {
            Self::MedianTime(u64::from(locktime))
        }
    }
    pub fn expired(self, point: &ChainPoint) -> bool {
        match self {
            Self::Height(height) => point.height >= height,
            Self::Timestamp(seconds) => point.timestamp >= seconds,
            Self::MedianTime(seconds) => point.timestamp > seconds,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainPoint {
    pub height: u64,
    pub hash: String,
    /// Bitcoin/Arkade median-time-past; EVM block timestamp. Never wall time.
    pub timestamp: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashLock {
    Sha256([u8; 32]),
    Hash160([u8; 20]),
}
impl HashLock {
    pub fn matches(self, preimage: &[u8]) -> bool {
        preimage.len() == 32
            && match self {
                Self::Sha256(hash) => sha256::Hash::hash(preimage).to_byte_array() == hash,
                Self::Hash160(hash) => hash160::Hash::hash(preimage).to_byte_array() == hash,
            }
    }
    pub fn preimage(self, bytes: &[u8]) -> Option<VerifiedPreimage> {
        self.matches(bytes)
            .then(|| VerifiedPreimage(bytes.try_into().expect("32 byte preimage")))
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedPreimage([u8; 32]);
impl VerifiedPreimage {
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}
impl std::fmt::Debug for VerifiedPreimage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerifiedPreimage([REDACTED])")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtlcState {
    Unknown,
    Absent,
    Mempool,
    Confirmed,
    Invalid,
    SpentClaim,
    SpentRefund,
    SpentUnknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub state: HtlcState,
    pub point: Option<ChainPoint>,
    /// Decimal base-unit amounts, preserving full EVM uint256 precision.
    pub funded_amount: String,
    pub remaining_amount: String,
    pub preimage: Option<VerifiedPreimage>,
}
impl Observation {
    pub fn unknown() -> Self {
        Self {
            state: HtlcState::Unknown,
            point: None,
            funded_amount: "0".into(),
            remaining_amount: "0".into(),
            preimage: None,
        }
    }
}
pub trait ContractReader: Send + Sync {
    fn can_observe(&self, reference: &ContractRef) -> bool;
    fn observe<'a>(&'a self, reference: &'a ContractRef) -> ObservationFuture<'a, Observation>;
}
fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidSwap(message.into())
}

#[cfg(test)]
mod tests;
