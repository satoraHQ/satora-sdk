use super::GetSwapResponse;
use serde::Deserialize;
use serde::Serialize;

/// A server-discovered swap and the wallet derivation index needed to resume it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveredSwap {
    pub index: u32,
    #[serde(flatten)]
    pub swap: GetSwapResponse,
}

/// A recovery page. Continue at `scanned_until + 1` when `complete` is false.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryResponse {
    pub swaps: Vec<RecoveredSwap>,
    pub highest_index: u32,
    pub next_index: u32,
    pub scanned_until: u32,
    pub complete: bool,
}
