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
        confirmations: Some(1),
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
        funding_uncertain: false,
        submission_uncertain: false,
        collaborative_refund_available: false,
        bridge: &BridgeState::None,
        server_status: None,
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
fn a_hashless_funding_reservation_blocks_only_before_the_client_lock_is_seen() {
    use HtlcState::*;
    let contracts = SwapContracts {
        client: Some(reference(200)),
        server: Some(reference(100)),
    };
    for (client, server, expected) in [
        (
            Absent,
            Absent,
            ActionKind::Wait {
                on: WaitingOn::SubmissionRecovery,
            },
        ),
        (Confirmed, Confirmed, ActionKind::Claim),
    ] {
        let client = observation(client, 50);
        let server = observation(server, 50);
        let mut input = input(&contracts, &client, &server);
        input.funding_uncertain = true;
        let actions = derive_actions(input, &ActionPolicy::default());
        assert_eq!(actions.recommended().unwrap().kind, expected);
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
        Some(SwapStatus::ClientRefunded)
    );
    let refunded = observation(HtlcState::SpentRefund, 150);
    assert_eq!(
        derive_actions(
            input(&contracts, &absent, &refunded),
            &ActionPolicy::default()
        )
        .recommended()
        .unwrap()
        .kind,
        ActionKind::Complete {
            outcome: Outcome::Refunded
        }
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

#[test]
fn lightning_source_past_the_refund_locktime_ends_refunded() {
    let contracts = SwapContracts {
        client: None,
        server: Some(reference(100)),
    };
    let unused = observation(HtlcState::Absent, 0);
    let derive = |height, claim_submitted, submission_uncertain| {
        let server = observation(HtlcState::Confirmed, height);
        let mut data = input(&contracts, &unused, &server);
        data.claim_submitted = claim_submitted;
        data.submission_uncertain = submission_uncertain;
        derive_actions(data, &ActionPolicy::default())
    };
    let refunded = ActionKind::Complete {
        outcome: Outcome::Refunded,
    };
    for (height, expected, at_risk) in [
        (50, ActionKind::Claim, true),
        (
            99,
            ActionKind::Wait {
                on: WaitingOn::LightningResolution,
            },
            true,
        ),
        (100, refunded.clone(), false),
        (150, refunded, false),
    ] {
        let actions = derive(height, false, false);
        assert_eq!(actions.recommended().unwrap().kind, expected, "{height}");
        assert_eq!(actions.at_risk, at_risk, "{height}");
        assert_eq!(actions.actions.len(), 1, "{height}");
    }
    assert_eq!(
        derive(100, true, false).recommended().unwrap().kind,
        ActionKind::Wait {
            on: WaitingOn::ClaimConfirmation
        }
    );
    assert_eq!(
        derive(100, false, true).recommended().unwrap().kind,
        ActionKind::Wait {
            on: WaitingOn::SubmissionRecovery
        }
    );
}

fn all_statuses() -> Vec<Option<SwapStatus>> {
    use SwapStatus::*;
    [
        Pending,
        ClientFundingSeen,
        ClientFunded,
        ClientRefunded,
        ServerFunded,
        ClientRedeeming,
        ClientRedeemed,
        ServerRedeemed,
        ClientFundedServerRefunded,
        ClientRefundedServerFunded,
        ClientRefundedServerRefunded,
        Expired,
        ClientInvalidFunded,
        ClientFundedTooLate,
        ServerWontFund,
        ClientRedeemedAndClientRefunded,
        Other("new".into()),
    ]
    .into_iter()
    .map(Some)
    .chain([None])
    .collect()
}

#[test]
fn lightning_source_waits_on_the_payment_or_the_server_and_never_funds() {
    use HtlcState::*;
    let contracts = SwapContracts {
        client: None,
        server: Some(reference(100)),
    };
    let unused = observation(Absent, 0);
    for (status, on) in [
        (None, WaitingOn::ClientPayment),
        (Some(SwapStatus::Pending), WaitingOn::ClientPayment),
        (Some(SwapStatus::ClientFunded), WaitingOn::ServerFunding),
    ] {
        for state in [Absent, Mempool, Invalid] {
            let server = observation(state, 50);
            let mut data = input(&contracts, &unused, &server);
            data.server_status = status.as_ref();
            let actions = derive_actions(data, &ActionPolicy::default());
            assert_eq!(
                actions.actions,
                wait(on),
                "{status:?} {state:?} should wait on {on:?}"
            );
            assert!(!actions.at_risk);
        }
    }
}

#[test]
fn lightning_source_failure_statuses_end_the_swap() {
    use HtlcState::*;
    use SwapStatus::*;
    let contracts = SwapContracts {
        client: None,
        server: Some(reference(100)),
    };
    let unused = observation(Absent, 0);
    let derive = |state, height, status: &SwapStatus| {
        let server = observation(state, height);
        let mut data = input(&contracts, &unused, &server);
        data.server_status = Some(status);
        derive_actions(data, &ActionPolicy::default())
    };
    for status in [
        ClientFundedServerRefunded,
        ServerWontFund,
        ClientFundedTooLate,
        ClientInvalidFunded,
    ] {
        // A leg inside the claim margin can no longer be claimed, so the status ends
        // the swap before the locktime does.
        for (state, height) in [(Absent, 50), (Invalid, 50), (Confirmed, 99)] {
            let actions = derive(state, height, &status);
            assert_eq!(
                actions.actions,
                complete(Outcome::Refunded),
                "{status:?} {state:?} {height}"
            );
            assert!(!actions.at_risk, "{status:?} {state:?} {height}");
        }
    }
    for state in [Absent, Invalid] {
        let actions = derive(state, 50, &Expired);
        assert_eq!(actions.actions, complete(Outcome::Expired), "{state:?}");
        assert!(!actions.at_risk);
    }
    // A funded server leg is chain evidence against Expired, so the locktime decides.
    assert_eq!(
        derive(Confirmed, 99, &Expired).actions,
        wait(WaitingOn::LightningResolution)
    );
    assert_eq!(
        derive(Confirmed, 100, &Expired).actions,
        complete(Outcome::Refunded)
    );
}

#[test]
fn server_status_never_authorizes_or_overrides_chain() {
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
        client: None,
        server: Some(reference(100)),
    };
    let unused = observation(Absent, 0);
    for status in all_statuses() {
        // Listed here rather than read from `lightning_source_failure`, so widening
        // that match fails the comparison below.
        let terminal = matches!(
            status,
            Some(
                SwapStatus::ClientFundedServerRefunded
                    | SwapStatus::ServerWontFund
                    | SwapStatus::ClientFundedTooLate
                    | SwapStatus::ClientInvalidFunded
                    | SwapStatus::Expired
            )
        );
        for state in states {
            for height in [50, 99, 100] {
                let server = observation(state, height);
                let derive =
                    |status: Option<&SwapStatus>, claim_submitted, submission_uncertain| {
                        let mut data = input(&contracts, &unused, &server);
                        data.server_status = status;
                        data.claim_submitted = claim_submitted;
                        data.submission_uncertain = submission_uncertain;
                        derive_actions(data, &ActionPolicy::default())
                    };
                let case = format!("{status:?} {state:?} {height}");
                for (claim_submitted, submission_uncertain) in
                    [(false, false), (true, false), (false, true), (true, true)]
                {
                    let case = format!("{case} {claim_submitted} {submission_uncertain}");
                    let actions = derive(status.as_ref(), claim_submitted, submission_uncertain);
                    let mut chain_only = derive(None, claim_submitted, submission_uncertain);
                    if terminal {
                        assert_eq!(actions.status, chain_only.status, "{case}");
                        if actions.actions != chain_only.actions {
                            assert!(
                                matches!(
                                    chain_only.recommended().unwrap().kind,
                                    ActionKind::Wait {
                                        on: WaitingOn::ClientPayment
                                            | WaitingOn::LightningResolution
                                    }
                                ),
                                "{case}: {chain_only:?}"
                            );
                            assert!(
                                matches!(
                                    actions.recommended().unwrap().kind,
                                    ActionKind::Complete {
                                        outcome: Outcome::Refunded | Outcome::Expired
                                    }
                                ),
                                "{case}: {actions:?}"
                            );
                        }
                    } else {
                        if status == Some(SwapStatus::ClientFunded) {
                            for action in &mut chain_only.actions {
                                if action.kind
                                    == (ActionKind::Wait {
                                        on: WaitingOn::ClientPayment,
                                    })
                                {
                                    action.kind = ActionKind::Wait {
                                        on: WaitingOn::ServerFunding,
                                    };
                                }
                            }
                        }
                        assert_eq!(actions, chain_only, "{case}");
                    }
                }
                let actions = derive(status.as_ref(), false, false);
                assert!(
                    actions.actions.iter().all(|a| matches!(
                        a.kind,
                        ActionKind::Claim | ActionKind::Wait { .. } | ActionKind::Complete { .. }
                    )),
                    "{case}: {actions:?}"
                );
                let kind = &actions.recommended().unwrap().kind;
                assert_eq!(
                    *kind == ActionKind::Claim,
                    state == Confirmed && height == 50,
                    "{case}"
                );
                let expected = match state {
                    Unknown | SpentUnknown => Some(ActionKind::Wait {
                        on: WaitingOn::Verification,
                    }),
                    SpentClaim => Some(ActionKind::Complete {
                        outcome: Outcome::Completed,
                    }),
                    SpentRefund => Some(ActionKind::Complete {
                        outcome: Outcome::Refunded,
                    }),
                    Absent | Mempool | Confirmed | Invalid => None,
                };
                if let Some(expected) = expected {
                    assert_eq!(*kind, expected, "{case}");
                }
                if state == Mempool {
                    assert!(
                        matches!(
                            kind,
                            ActionKind::Wait {
                                on: WaitingOn::ClientPayment | WaitingOn::ServerFunding
                            }
                        ),
                        "{case}"
                    );
                }
                if !matches!(state, SpentClaim | SpentRefund) {
                    assert_eq!(
                        derive(status.as_ref(), false, true).actions,
                        wait(WaitingOn::SubmissionRecovery),
                        "{case}"
                    );
                }
                if state == Confirmed {
                    assert_eq!(
                        derive(status.as_ref(), true, false).actions,
                        wait(WaitingOn::ClaimConfirmation),
                        "{case}"
                    );
                }
            }
        }
    }
}

#[test]
fn server_status_is_ignored_with_a_client_leg() {
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
    for status in all_statuses() {
        for c in states {
            for s in states {
                for height in [50, 99, 200] {
                    let client = observation(c, height);
                    let server = observation(s, height);
                    let mut data = input(&contracts, &client, &server);
                    data.server_status = status.as_ref();
                    assert_eq!(
                        derive_actions(data, &ActionPolicy::default()),
                        derive_actions(
                            input(&contracts, &client, &server),
                            &ActionPolicy::default()
                        ),
                        "{status:?} {c:?} {s:?} {height}"
                    );
                }
            }
        }
    }
}

#[test]
fn cctp_recovery_follows_expiry_grace_and_verification() {
    use HtlcState::*;
    let contracts = SwapContracts {
        client: Some(reference(200)),
        server: Some(reference(100)),
    };
    let attested = BridgeState::CctpBurn {
        source_transaction: "burn".into(),
        attestation_available: true,
    };
    let unattested = BridgeState::CctpBurn {
        source_transaction: "burn".into(),
        attestation_available: false,
    };
    let recommend = |client: Observation, server: Observation, bridge: &BridgeState, submitted| {
        let mut data = input(&contracts, &client, &server);
        data.bridge = bridge;
        data.funding_submitted = submitted;
        derive_actions(data, &ActionPolicy::default())
            .recommended()
            .unwrap()
            .kind
            .clone()
    };
    let server = || observation(Absent, 50);
    assert_eq!(
        recommend(observation(Absent, 50), server(), &attested, false),
        ActionKind::RecoverCctp {
            source_transaction: "burn".into()
        }
    );
    assert_eq!(
        recommend(observation(Absent, 200), server(), &attested, false),
        ActionKind::Wait {
            on: WaitingOn::Verification
        }
    );
    assert_eq!(
        recommend(observation(Absent, 206), server(), &attested, false),
        ActionKind::Complete {
            outcome: Outcome::Expired
        }
    );
    assert_eq!(
        recommend(observation(Absent, 206), server(), &unattested, false),
        ActionKind::Complete {
            outcome: Outcome::Expired
        }
    );
    assert_eq!(
        recommend(observation(Absent, 50), server(), &unattested, false),
        ActionKind::Wait {
            on: WaitingOn::BridgeAttestation
        }
    );
    assert_eq!(
        recommend(
            observation(Absent, 50),
            observation(Unknown, 50),
            &attested,
            false
        ),
        ActionKind::Wait {
            on: WaitingOn::Verification
        }
    );
    assert_eq!(
        recommend(observation(Absent, 206), server(), &attested, true),
        ActionKind::Wait {
            on: WaitingOn::FundingConfirmation
        }
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
#[test]
fn only_confirmations_manual_steps_and_submission_recovery_need_the_user() {
    use HtlcState::*;
    let contracts = SwapContracts {
        client: Some(reference(200)),
        server: Some(reference(100)),
    };
    let policy = ActionPolicy::default();
    let derive = |client, server, height, uncertain| {
        let (client, server) = (observation(client, height), observation(server, height));
        derive_actions(
            ActionInput {
                submission_uncertain: uncertain,
                ..input(&contracts, &client, &server)
            },
            &policy,
        )
    };
    for (client, server, height, uncertain, needs_user) in [
        (Absent, Absent, 50, false, true),
        (Confirmed, Absent, 50, false, false),
        (Confirmed, Confirmed, 50, false, false),
        (Confirmed, Confirmed, 50, true, true),
        (Confirmed, SpentRefund, 200, false, true),
        (Confirmed, SpentClaim, 50, false, false),
    ] {
        let actions = derive(client, server, height, uncertain);
        assert_eq!(actions.needs_user(), needs_user, "{actions:?}");
    }
}
