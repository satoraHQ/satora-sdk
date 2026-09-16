//! Pure action derivation from chain observations. All clocks and capabilities
//! are explicit inputs; server status never authorizes a payment or claim.
use crate::observation::ChainPoint;
use crate::observation::HtlcState;
use crate::observation::Observation;
use crate::observation::SwapContracts;
use crate::observation::Timelock;
use crate::types::SwapStatus;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Automation {
    Automatic,
    Confirm,
    Manual,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitingOn {
    Verification,
    ClientPayment,
    FundingConfirmation,
    ServerFunding,
    ClaimConfirmation,
    RefundTimelock,
    BridgeAttestation,
    BridgeDelivery,
    SubmissionRecovery,
    LightningResolution,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    Refunded,
    Expired,
    Anomalous,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionKind {
    Fund,
    Claim,
    RefundCollaborative,
    RefundUnilateral,
    RecoverCctp { source_transaction: String },
    RecoverBridge,
    Wait { on: WaitingOn },
    Complete { outcome: Outcome },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub kind: ActionKind,
    pub recommended: bool,
    pub automation: Automation,
    /// A refund can be displayed before it becomes executable.
    pub blocked_by_timelock: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapActions {
    pub status: Option<SwapStatus>,
    pub actions: Vec<Action>,
    pub at_risk: bool,
}
impl SwapActions {
    pub fn recommended(&self) -> Option<&Action> {
        self.actions.iter().find(|action| action.recommended)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BridgeState {
    #[default]
    None,
    CctpBurn {
        source_transaction: String,
        attestation_available: bool,
    },
    OutboundPending,
    OutboundFailed,
    Delivered,
}
#[derive(Clone, Debug)]
pub struct ActionPolicy {
    pub claim_margin_seconds: u64,
    pub claim_margin_blocks: u64,
    pub funding_grace_seconds: u64,
    pub funding_grace_blocks: u64,
}
impl Default for ActionPolicy {
    fn default() -> Self {
        Self {
            claim_margin_seconds: 300,
            claim_margin_blocks: 1,
            funding_grace_seconds: 3600,
            funding_grace_blocks: 6,
        }
    }
}
pub struct ActionInput<'a> {
    pub contracts: &'a SwapContracts,
    pub client: Option<&'a Observation>,
    pub server: Option<&'a Observation>,
    pub funding_submitted: bool,
    pub claim_submitted: bool,
    pub submission_uncertain: bool,
    /// Application-verified Arkade capability; ignored for other client rails.
    pub collaborative_refund_available: bool,
    pub bridge: &'a BridgeState,
}

pub fn derive_status(client: Option<HtlcState>, server: Option<HtlcState>) -> Option<SwapStatus> {
    use HtlcState::*;
    use SwapStatus as S;
    if client
        .into_iter()
        .chain(server)
        .any(|state| matches!(state, Unknown | SpentUnknown))
    {
        return None;
    }
    Some(match (client, server) {
        (Some(state), None) => match state {
            Absent => S::Pending,
            Mempool => S::ClientFundingSeen,
            Confirmed => S::ClientFunded,
            Invalid => S::ClientInvalidFunded,
            SpentClaim => S::ServerRedeemed,
            SpentRefund => S::ClientRefunded,
            _ => return None,
        },
        (None, Some(state)) => match state {
            SpentClaim => S::ClientRedeemed,
            Confirmed => S::ServerFunded,
            SpentRefund => S::Expired,
            _ => S::Pending,
        },
        (Some(SpentClaim), Some(SpentClaim)) => S::ServerRedeemed,
        (Some(SpentRefund), Some(SpentClaim)) => S::ClientRedeemedAndClientRefunded,
        (Some(SpentRefund), Some(SpentRefund)) => S::ClientRefundedServerRefunded,
        (Some(SpentRefund), Some(Confirmed)) => S::ClientRefundedServerFunded,
        (Some(SpentRefund), Some(Absent | Mempool)) => S::ClientRefunded,
        (Some(Confirmed), Some(SpentClaim)) => S::ClientRedeemed,
        (Some(Confirmed), Some(SpentRefund)) => S::ClientFundedServerRefunded,
        (Some(Invalid), Some(_)) => S::ClientInvalidFunded,
        (Some(Absent), Some(Absent | Mempool | Confirmed | Invalid)) => S::Pending,
        (Some(Mempool), Some(Absent | Mempool | Confirmed | Invalid)) => S::ClientFundingSeen,
        (Some(Confirmed), Some(Confirmed)) => S::ServerFunded,
        (Some(Confirmed), Some(Absent | Mempool | Invalid)) => S::ClientFunded,
        _ => return None,
    })
}
fn action(kind: ActionKind, automation: Automation) -> Action {
    Action {
        kind,
        recommended: true,
        automation,
        blocked_by_timelock: false,
    }
}
fn wait(on: WaitingOn) -> Vec<Action> {
    vec![action(ActionKind::Wait { on }, Automation::Automatic)]
}
fn complete(outcome: Outcome) -> Vec<Action> {
    vec![action(
        ActionKind::Complete { outcome },
        Automation::Automatic,
    )]
}
fn expired(
    reference: Option<&crate::observation::ContractRef>,
    observation: Option<&Observation>,
) -> Option<bool> {
    Some(reference?.timelock().expired(observation?.point.as_ref()?))
}
fn before_claim_margin(lock: Timelock, point: &ChainPoint, policy: &ActionPolicy) -> bool {
    match lock {
        Timelock::Height(height) => point
            .height
            .checked_add(policy.claim_margin_blocks)
            .is_some_and(|now| now < height),
        Timelock::Timestamp(seconds) | Timelock::MedianTime(seconds) => point
            .timestamp
            .checked_add(policy.claim_margin_seconds)
            .is_some_and(|now| now < seconds),
    }
}
fn past_grace(lock: Timelock, point: &ChainPoint, policy: &ActionPolicy) -> bool {
    match lock {
        Timelock::Height(height) => {
            point.height >= height.saturating_add(policy.funding_grace_blocks)
        }
        Timelock::Timestamp(seconds) | Timelock::MedianTime(seconds) => {
            point.timestamp >= seconds.saturating_add(policy.funding_grace_seconds)
        }
    }
}
fn refunds(input: &ActionInput<'_>, waiting: WaitingOn) -> Vec<Action> {
    if input.contracts.client.is_none() {
        return wait(WaitingOn::LightningResolution);
    }
    match expired(input.contracts.client.as_ref(), input.client) {
        Some(true) => vec![action(ActionKind::RefundUnilateral, Automation::Confirm)],
        Some(false) => {
            let mut actions = if input.collaborative_refund_available
                && matches!(
                    input.contracts.client,
                    Some(crate::observation::ContractRef::Arkade { .. })
                ) {
                vec![action(ActionKind::RefundCollaborative, Automation::Confirm)]
            } else {
                wait(waiting)
            };
            actions.push(Action {
                kind: ActionKind::RefundUnilateral,
                recommended: false,
                automation: Automation::Confirm,
                blocked_by_timelock: true,
            });
            actions
        }
        None => wait(WaitingOn::Verification),
    }
}
pub fn derive_actions(input: ActionInput<'_>, policy: &ActionPolicy) -> SwapActions {
    let client = input
        .contracts
        .client
        .as_ref()
        .map(|_| input.client.map_or(HtlcState::Unknown, |o| o.state));
    let server = input
        .contracts
        .server
        .as_ref()
        .map(|_| input.server.map_or(HtlcState::Unknown, |o| o.state));
    let status = derive_status(client, server);
    let at_risk = client.is_some_and(|state| {
        !matches!(
            state,
            HtlcState::Absent | HtlcState::SpentClaim | HtlcState::SpentRefund
        )
    }) || server.is_some_and(|state| {
        matches!(
            state,
            HtlcState::Confirmed | HtlcState::Unknown | HtlcState::SpentUnknown
        )
    }) || !matches!(input.bridge, BridgeState::None | BridgeState::Delivered);
    let actions = (|| {
        use SwapStatus::*;
        if matches!(
            status,
            Some(ClientRedeemed | ServerRedeemed | ClientRedeemedAndClientRefunded)
        ) {
            return match input.bridge {
                BridgeState::OutboundPending => wait(WaitingOn::BridgeDelivery),
                BridgeState::OutboundFailed => {
                    vec![action(ActionKind::RecoverBridge, Automation::Manual)]
                }
                _ => complete(if status == Some(ClientRedeemedAndClientRefunded) {
                    Outcome::Anomalous
                } else {
                    Outcome::Completed
                }),
            };
        }
        if matches!(
            status,
            Some(ClientRefunded | ClientRefundedServerRefunded | ClientRefundedServerFunded)
        ) {
            return complete(Outcome::Refunded);
        }
        if status == Some(Expired) {
            return complete(Outcome::Expired);
        }
        if input.submission_uncertain {
            return wait(WaitingOn::SubmissionRecovery);
        }
        if let BridgeState::CctpBurn {
            source_transaction,
            attestation_available,
        } = input.bridge
            && matches!(client, Some(HtlcState::Absent))
        {
            if input.funding_submitted {
                return wait(WaitingOn::FundingConfirmation);
            }
            return if *attestation_available {
                vec![action(
                    ActionKind::RecoverCctp {
                        source_transaction: source_transaction.clone(),
                    },
                    Automation::Automatic,
                )]
            } else {
                wait(WaitingOn::BridgeAttestation)
            };
        }
        match status {
            None => wait(WaitingOn::Verification),
            Some(Pending) => {
                if input.contracts.client.is_none() {
                    return wait(WaitingOn::ClientPayment);
                }
                if input.funding_submitted {
                    return wait(WaitingOn::FundingConfirmation);
                }
                match expired(input.contracts.client.as_ref(), input.client) {
                    Some(false) => vec![action(ActionKind::Fund, Automation::Manual)],
                    Some(true) => {
                        let reference = input.contracts.client.as_ref().unwrap();
                        let point = input.client.and_then(|o| o.point.as_ref()).unwrap();
                        if past_grace(reference.timelock(), point, policy) {
                            complete(Outcome::Expired)
                        } else {
                            wait(WaitingOn::Verification)
                        }
                    }
                    None => wait(WaitingOn::Verification),
                }
            }
            Some(ClientFundingSeen) => wait(WaitingOn::FundingConfirmation),
            Some(ClientFunded) => refunds(&input, WaitingOn::ServerFunding),
            Some(ServerFunded) => {
                if input.claim_submitted {
                    return wait(WaitingOn::ClaimConfirmation);
                }
                let Some(reference) = &input.contracts.server else {
                    return refunds(&input, WaitingOn::ServerFunding);
                };
                let Some(point) = input.server.and_then(|o| o.point.as_ref()) else {
                    return wait(WaitingOn::Verification);
                };
                if before_claim_margin(reference.timelock(), point, policy) {
                    vec![action(ActionKind::Claim, Automation::Automatic)]
                } else {
                    refunds(&input, WaitingOn::RefundTimelock)
                }
            }
            Some(
                ClientFundedServerRefunded
                | ClientInvalidFunded
                | ClientFundedTooLate
                | ServerWontFund,
            ) => refunds(&input, WaitingOn::RefundTimelock),
            _ => wait(WaitingOn::Verification),
        }
    })();
    SwapActions {
        status,
        actions,
        at_risk,
    }
}

#[cfg(test)]
pub(crate) mod tests;
