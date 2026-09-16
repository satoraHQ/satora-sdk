use super::*;
use crate::bitcoin::BitcoinHtlc;
use crate::observation::ContractRef;
use bitcoin::secp256k1::Keypair;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::secp256k1::SecretKey;
pub(crate) fn reference(locktime: u32) -> ContractRef {
    let key =
        Keypair::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(&[1; 32]).unwrap())
            .x_only_public_key()
            .0;
    ContractRef::Bitcoin {
        htlc: BitcoinHtlc {
            network: bitcoin::Network::Regtest,
            hash_lock: [42; 20],
            claimer: key,
            refunder: key,
            refund_locktime: locktime,
            script_version: 1,
        },
        expected_sats: 1000,
        confirmations: 1,
    }
}
pub(crate) fn observation(state: HtlcState, height: u64) -> Observation {
    Observation {
        state,
        point: Some(ChainPoint {
            height,
            hash: "tip".into(),
            timestamp: 600_000_000,
        }),
        funded_amount: "1000".into(),
        remaining_amount: "1000".into(),
        preimage: None,
    }
}
fn input<'a>(
    contracts: &'a SwapContracts,
    client: &'a Observation,
    server: &'a Observation,
) -> ActionInput<'a> {
    ActionInput {
        contracts,
        client: Some(client),
        server: Some(server),
        funding_submitted: false,
        claim_submitted: false,
        submission_uncertain: false,
        collaborative_refund_available: false,
        bridge: &BridgeState::None,
    }
}
#[test]
fn funding_claim_and_refund_transitions_use_confirmed_state_and_chain_clock() {
    use HtlcState::*;
    let contracts = SwapContracts {
        client: Some(reference(200)),
        server: Some(reference(100)),
    };
    let policy = ActionPolicy::default();
    let cases = [
        (Absent, Absent, 50, ActionKind::Fund),
        (
            Mempool,
            Absent,
            50,
            ActionKind::Wait {
                on: WaitingOn::FundingConfirmation,
            },
        ),
        (
            Confirmed,
            Absent,
            50,
            ActionKind::Wait {
                on: WaitingOn::ServerFunding,
            },
        ),
        (
            Confirmed,
            Invalid,
            50,
            ActionKind::Wait {
                on: WaitingOn::ServerFunding,
            },
        ),
        (Confirmed, Confirmed, 50, ActionKind::Claim),
        (
            Confirmed,
            Confirmed,
            99,
            ActionKind::Wait {
                on: WaitingOn::RefundTimelock,
            },
        ),
        (Confirmed, SpentRefund, 200, ActionKind::RefundUnilateral),
        (
            Confirmed,
            SpentClaim,
            50,
            ActionKind::Complete {
                outcome: Outcome::Completed,
            },
        ),
        (
            SpentRefund,
            SpentClaim,
            50,
            ActionKind::Complete {
                outcome: Outcome::Anomalous,
            },
        ),
        (
            SpentRefund,
            Absent,
            200,
            ActionKind::Complete {
                outcome: Outcome::Refunded,
            },
        ),
        (
            Absent,
            Absent,
            205,
            ActionKind::Wait {
                on: WaitingOn::Verification,
            },
        ),
        (
            Absent,
            Absent,
            206,
            ActionKind::Complete {
                outcome: Outcome::Expired,
            },
        ),
    ];
    for (client, server, height, expected) in cases {
        let client = observation(client, height);
        let server = observation(server, height);
        let actions = derive_actions(input(&contracts, &client, &server), &policy);
        assert_eq!(actions.recommended().unwrap().kind, expected);
        if expected == ActionKind::Fund {
            assert_eq!(
                actions.recommended().unwrap().automation,
                Automation::Manual
            );
        }
        if expected == ActionKind::RefundUnilateral {
            assert_eq!(
                actions.recommended().unwrap().automation,
                Automation::Confirm
            );
        }
    }
}
#[test]
fn unknown_contradictory_and_missing_clocks_never_authorize_claims() {
    use HtlcState::*;
    let states = [
        Unknown,
        Absent,
        Mempool,
        Confirmed,
        Invalid,
        SpentClaim,
        SpentRefund,
        SpentUnknown,
    ];
    let contracts = SwapContracts {
        client: Some(reference(200)),
        server: Some(reference(100)),
    };
    for c in states {
        for s in states {
            let client = observation(c, 50);
            let server = observation(s, 50);
            let actions = derive_actions(
                input(&contracts, &client, &server),
                &ActionPolicy::default(),
            );
            if actions.recommended().unwrap().kind == ActionKind::Claim {
                assert_eq!((c, s), (Confirmed, Confirmed));
            }
            if matches!(c, Unknown | SpentUnknown) || matches!(s, Unknown | SpentUnknown) {
                assert_eq!(actions.status, None);
            }
        }
    }
    let client = observation(Confirmed, 50);
    let mut server = observation(Confirmed, 50);
    server.point = None;
    let result = derive_actions(
        input(&contracts, &client, &server),
        &ActionPolicy::default(),
    );
    assert_eq!(
        result.recommended().unwrap().kind,
        ActionKind::Wait {
            on: WaitingOn::Verification
        }
    );
    assert_eq!(derive_status(Some(Absent), Some(SpentClaim)), None);
}
#[test]
fn submitted_claims_bridges_and_lightning_are_not_assumed_complete() {
    let mut contracts = SwapContracts {
        client: Some(reference(200)),
        server: Some(reference(100)),
    };
    let client = observation(HtlcState::Confirmed, 50);
    let server = observation(HtlcState::Confirmed, 50);
    let mut data = input(&contracts, &client, &server);
    data.claim_submitted = true;
    assert_eq!(
        derive_actions(data, &ActionPolicy::default())
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Wait {
            on: WaitingOn::ClaimConfirmation
        }
    );
    let claimed = observation(HtlcState::SpentClaim, 50);
    let mut data = input(&contracts, &client, &claimed);
    data.bridge = &BridgeState::OutboundPending;
    assert_eq!(
        derive_actions(data, &ActionPolicy::default())
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Wait {
            on: WaitingOn::BridgeDelivery
        }
    );
    let absent = observation(HtlcState::Absent, 50);
    let bridge = BridgeState::CctpBurn {
        source_transaction: "burn".into(),
        attestation_available: true,
    };
    let mut data = input(&contracts, &absent, &absent);
    data.bridge = &bridge;
    assert!(matches!(
        derive_actions(data, &ActionPolicy::default())
            .recommended()
            .unwrap()
            .kind,
        ActionKind::RecoverCctp { .. }
    ));
    contracts.client = None;
    assert_eq!(
        derive_actions(
            input(&contracts, &absent, &server),
            &ActionPolicy::default()
        )
        .recommended()
        .unwrap()
        .kind,
        ActionKind::Claim
    );
    assert_eq!(
        derive_status(None, Some(HtlcState::SpentRefund)),
        Some(SwapStatus::Expired)
    );
    contracts.client = Some(reference(200));
    contracts.server = None;
    assert_eq!(
        derive_status(Some(HtlcState::SpentClaim), None),
        Some(SwapStatus::ServerRedeemed)
    );
    assert!(
        derive_actions(
            input(&contracts, &client, &server),
            &ActionPolicy::default()
        )
        .actions
        .iter()
        .all(|a| a.kind != ActionKind::Claim)
    );
}

#[cfg(feature = "gasless")]
#[test]
fn evm_refunds_ignore_legacy_cooperative_capability_and_wait_for_maturity() {
    use crate::evm::Address;
    use crate::evm::B256;
    use crate::evm::U256;
    let deadline = 600_000_200;
    let contracts = SwapContracts {
        client: Some(ContractRef::Evm {
            htlc: crate::evm::htlc::EvmHtlc {
                chain_id: 42161,
                address: Address::repeat_byte(1),
                hash_lock: B256::repeat_byte(2),
                amount: U256::from(1000),
                token: Address::repeat_byte(3),
                sender: Address::repeat_byte(4),
                claimer: Address::repeat_byte(5),
                timelock: deadline,
            },
            from_block: 0,
            confirmations: 1,
        }),
        server: Some(reference(100)),
    };
    for (timestamp, expected) in [
        (
            deadline - 1,
            ActionKind::Wait {
                on: WaitingOn::RefundTimelock,
            },
        ),
        (deadline, ActionKind::RefundUnilateral),
    ] {
        let mut client = observation(HtlcState::Confirmed, 150);
        client.point.as_mut().unwrap().timestamp = timestamp;
        let server = observation(HtlcState::SpentRefund, 150);
        let mut data = input(&contracts, &client, &server);
        data.collaborative_refund_available = true;
        let result = derive_actions(data, &ActionPolicy::default());
        assert_eq!(result.recommended().unwrap().kind, expected);
        assert!(
            result
                .actions
                .iter()
                .all(|a| a.kind != ActionKind::RefundCollaborative)
        );
    }
}

#[test]
fn arkade_still_offers_verified_cooperative_refunds() {
    use bitcoin::hashes::Hash;
    let ContractRef::Bitcoin { htlc, .. } = reference(200) else {
        unreachable!()
    };
    let contracts = SwapContracts {
        client: Some(ContractRef::Arkade {
            options: crate::arkade::StrictVhtlcOptions {
                sender: htlc.refunder,
                receiver: htlc.claimer,
                server: htlc.claimer,
                preimage_hash: bitcoin::hashes::ripemd160::Hash::from_byte_array(htlc.hash_lock),
                refund_locktime: 200,
                unilateral_claim_delay: bitcoin::Sequence::from_height(10),
                unilateral_refund_delay: bitcoin::Sequence::from_height(20),
                unilateral_refund_without_receiver_delay: bitcoin::Sequence::from_height(30),
            },
            network: bitcoin::Network::Regtest,
            expected_sats: 1000,
        }),
        server: Some(reference(100)),
    };
    let client = observation(HtlcState::Confirmed, 150);
    let server = observation(HtlcState::SpentRefund, 150);
    let mut data = input(&contracts, &client, &server);
    data.collaborative_refund_available = true;
    assert_eq!(
        derive_actions(data, &ActionPolicy::default())
            .recommended()
            .unwrap()
            .kind,
        ActionKind::RefundCollaborative
    );
}
