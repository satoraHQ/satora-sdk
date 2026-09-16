use super::*;
use crate::Error;
use crate::control::PollOptions;
use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::U256;
use cctp::*;
use serde_json::json;
use std::time::Duration;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

pub(crate) fn burn() -> CctpBurn {
    CctpBurn {
        source_chain_id: 8453,
        source_domain: 6,
        destination_domain: 3,
        source_messenger: TOKEN_MESSENGER,
        destination_messenger: TOKEN_MESSENGER.into_word(),
        source_transmitter: MESSAGE_TRANSMITTER,
        burn_token: CctpChain::for_chain(8453).unwrap().usdc,
        sender: Address::repeat_byte(1),
        mint_recipient: Address::repeat_byte(2).into_word(),
        destination_caller: B256::ZERO,
        amount: U256::from(10_000_000),
        max_fee: U256::from(1000),
        min_finality: 1000,
        hook: forwarding_hook(None),
    }
}
pub(crate) fn message(b: &CctpBurn) -> Vec<u8> {
    let mut result = Vec::new();
    result.extend(1u32.to_be_bytes());
    result.extend(b.source_domain.to_be_bytes());
    result.extend(b.destination_domain.to_be_bytes());
    result.extend([7; 32]);
    result.extend(b.source_messenger.into_word());
    result.extend(b.destination_messenger);
    result.extend(b.destination_caller);
    result.extend(b.min_finality.to_be_bytes());
    result.extend(1000u32.to_be_bytes());
    result.extend(1u32.to_be_bytes());
    result.extend(b.burn_token.into_word());
    result.extend(b.mint_recipient);
    result.extend(b.amount.to_be_bytes::<32>());
    result.extend(b.sender.into_word());
    result.extend(b.max_fee.to_be_bytes::<32>());
    result.extend(U256::from(100).to_be_bytes::<32>());
    result.extend(U256::from(123).to_be_bytes::<32>());
    result.extend(b.hook.iter());
    result
}
fn attested(b: &CctpBurn) -> serde_json::Value {
    json!({"message":format!("0x{}",hex::encode(message(b))),"attestation":"0x1234","status":"complete"})
}
#[test]
fn cctp_layout_rejects_changed_immutable_fields_and_excess_fees() {
    let burn = burn();
    let original = message(&burn);
    assert_eq!(
        burn.validate_message(&original).unwrap().minted_amount(),
        U256::from(9_999_900)
    );
    for index in [0, 4, 8, 44, 76, 108, 140, 148, 152, 184, 216, 248, 280, 376] {
        let mut mutated = original.clone();
        mutated[index] ^= 1;
        assert!(burn.validate_message(&mutated).is_err(), "offset {index}");
    }
    let mut mutable = original.clone();
    mutable[12] ^= 1;
    mutable[344] ^= 1;
    assert!(burn.validate_message(&mutable).is_ok()); // Circle assigns these after the burn.
    let mut too_expensive = original.clone();
    too_expensive[312..344].copy_from_slice(&U256::from(1001).to_be_bytes::<32>());
    assert!(burn.validate_message(&too_expensive).is_err());
    for length in [0, 12, 147, 375] {
        assert!(CctpMessage::decode(&original[..length]).is_err());
    }
}
#[test]
fn registries_fees_and_solana_hooks_match_the_reference() {
    assert_eq!(CHAINS.len(), 15);
    assert_eq!(usdt0::CHAINS.len(), 22);
    assert_eq!(CctpChain::for_chain(50).unwrap().finality(), 2000);
    assert!(CctpChain::for_chain(8453).unwrap().requires_bridge());
    assert!(!CctpChain::for_chain(42161).unwrap().requires_bridge());
    let hook = forwarding_hook(Some([42; 32]));
    assert_eq!(hook.len(), 65);
    assert_eq!(&hook[28..33], &[0, 0, 0, 33, 1]);
    assert_eq!(&hook[33..], &[42; 32]);
    let fee: CctpFee = serde_json::from_value(
        json!({"finalityThreshold":1000,"minimumFee":1.3,"forwardFee":{"high":200000}}),
    )
    .unwrap();
    assert_eq!(
        fee.max_fee(U256::from(100_000_000), false).unwrap(),
        U256::from(15600)
    );
    assert_eq!(
        fee.max_fee(U256::from(100_000_000), true).unwrap(),
        U256::from(215600)
    );
    assert!(fee.max_fee(U256::MAX, false).is_err());
    assert_ne!(
        usdt0::ARBITRUM_OFT,
        usdt0::CHAINS
            .iter()
            .find(|c| c.name == "Arbitrum")
            .unwrap()
            .token
    );
}
#[tokio::test]
async fn attestation_selection_timeout_and_cancellation() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &server.uri()).unwrap();
    let expected = burn();
    let mut other = expected.clone();
    other.mint_recipient = B256::repeat_byte(9);
    Mock::given(path("/v2/messages/6"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"messages":[attested(&other),attested(&expected)]})),
        )
        .mount(&server)
        .await;
    let found = api
        .cctp_attestation(B256::repeat_byte(4), &expected)
        .await
        .unwrap()
        .unwrap();
    found.receive_calldata(&expected).unwrap();
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
        .mount(&server)
        .await;
    let options = PollOptions {
        timeout: Duration::from_millis(40),
        interval: Duration::from_millis(1),
        ..Default::default()
    };
    assert!(matches!(
        api.wait_cctp_attestation(B256::repeat_byte(4), &expected, &options)
            .await,
        Err(Error::Timeout(_))
    ));
    options.cancellation.cancel();
    assert!(matches!(
        api.wait_cctp_attestation(B256::repeat_byte(4), &expected, &options)
            .await,
        Err(Error::Cancelled)
    ));
}
#[tokio::test]
async fn layerzero_requires_matching_guid_route_and_delivered_transaction() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &server.uri()).unwrap();
    let query = usdt0::MessageQuery {
        source_txid: B256::repeat_byte(3),
        source_eid: 30110,
        destination_eid: 30111,
        guid: B256::repeat_byte(4),
    };
    let entry = |guid, status: &str, dst| json!({"guid":guid,"status":{"name":status},"pathway":{"srcEid":30110,"dstEid":30111},"source":{"tx":{"txHash":query.source_txid}},"destination":{"tx":dst}});
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[entry(B256::repeat_byte(9),"DELIVERED",json!({"txHash":B256::repeat_byte(7)})),entry(query.guid,"DELIVERED",json!({"txHash":B256::repeat_byte(8)}))]}))).mount(&server).await;
    assert_eq!(
        api.usdt0_status(&query)
            .await
            .unwrap()
            .unwrap()
            .destination_txid,
        Some(B256::repeat_byte(8))
    );
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"data":[entry(query.guid,"BLOCKED",serde_json::Value::Null)]}),
            ),
        )
        .mount(&server)
        .await;
    assert!(matches!(
        api.wait_usdt0(&query, &PollOptions::default()).await,
        Err(Error::InvalidSwap(_))
    ));
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(400))
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        api.wait_usdt0(&query, &PollOptions::default()).await,
        Err(Error::Api { status: 400, .. })
    ));
}
