//! Ported vectors from
//! ts-pure-sdk/tests/compose-{quote-direct-pegged,net-target,quote-layerzero-fee}.test.ts.
#[path = "support/quotes.rs"]
mod quotes;
use lendaswap_sdk::Client;
use lendaswap_sdk::types::*;
use quotes::*;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[tokio::test]
async fn direct_pivot_scales_both_directions_and_pinned_sides_without_dex() {
    let server = MockServer::start().await;
    let client = Client::new(&server.uri()).unwrap();
    let (pairs, fees, config) = data();
    for (btc_source, amount) in [
        (true, QuoteAmount::Source(100_000)),
        (true, QuoteAmount::Target(1_000_000_000_000_000)),
        (false, QuoteAmount::Source(1_000_000_000_000_000)),
        (false, QuoteAmount::Target(100_000)),
    ] {
        let q = client
            .compose_quote_with_data(
                request(btc_source, Chain::arbitrum(), TBTC, amount),
                100,
                &pairs,
                &fees,
                &config,
                0.0,
            )
            .await
            .unwrap();
        let (source, target) = if btc_source {
            ("100000", "1000000000000000")
        } else {
            ("1000000000000000", "100000")
        };
        assert_eq!(q.source_amount, source);
        assert_eq!(q.target_amount, target);
        assert_eq!(q.net_source_amount, source);
        assert_eq!(q.net_target_amount, target);
        assert_eq!(q.exchange_rate, "1");
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn non_unit_peg_rounding_dust_and_protocol_fees_match_typescript() {
    let client = Client::new("http://localhost:1").unwrap();
    let (mut pairs, mut fees, config) = data();
    let q = client
        .compose_quote_with_data(
            request(true, Chain::polygon(), WBTC, QuoteAmount::Source(100_000)),
            100,
            &pairs,
            &fees,
            &config,
            0.0,
        )
        .await
        .unwrap();
    assert_eq!(q.target_amount, "99790");
    assert_eq!(q.exchange_rate, "0.9979");
    let q = client
        .compose_quote_with_data(
            request(
                false,
                Chain::arbitrum(),
                TBTC,
                QuoteAmount::Source(5_000_000_000),
            ),
            100,
            &pairs,
            &fees,
            &config,
            0.0,
        )
        .await
        .unwrap();
    assert_eq!(q.target_amount, "0");
    assert_eq!(q.exchange_rate, "0");
    pairs.pairs[0].fee_percentage = 0.0025;
    fees.pairs[0].fees = NetworkFee {
        source_sats: 500,
        target_sats: 300,
    };
    let q = client
        .compose_quote_with_data(
            request(true, Chain::arbitrum(), TBTC, QuoteAmount::Source(100_000)),
            100,
            &pairs,
            &fees,
            &config,
            0.0,
        )
        .await
        .unwrap();
    assert_eq!(q.network_fee, 800);
    assert_eq!(q.protocol_fee, 250);
    assert_eq!(q.net_target_amount, "989500000000000");
    let q = client
        .compose_quote_with_data(
            request(
                true,
                Chain::arbitrum(),
                TBTC,
                QuoteAmount::Target(1_000_000_000_000_000),
            ),
            100,
            &pairs,
            &fees,
            &config,
            0.001,
        )
        .await
        .unwrap();
    assert_eq!(q.protocol_fee, 350);
    assert_eq!(q.net_source_amount, "101150");
}

#[tokio::test]
async fn bridge_fee_is_subtracted_at_full_value_and_solana_setup_is_forwarded() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/dex-quote"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"expected_amount_in":{"raw":"10000000000000000","decimals":18},"estimated_amount_out":{"raw":"990000","decimals":6},"bridge_fee":"10000","estimated_settlement_seconds":15,"router":"cctp","cache_ttl_seconds":30}))).mount(&server).await;
    let client = Client::new(&server.uri()).unwrap();
    let (mut pairs, fees, config) = data();
    pairs.pairs[0].fee_percentage = 0.01;
    let mut request = request(
        true,
        Chain::Other("Solana".into()),
        "solana-mint",
        QuoteAmount::Source(1_000_000),
    );
    request.bridge_recipient_setup = true;
    let q = client
        .compose_quote_with_data(request, 100, &pairs, &fees, &config, 0.0)
        .await
        .unwrap();
    assert_eq!(q.target_amount, "1000000");
    assert_eq!(q.net_target_amount, "980000");
    assert_eq!(q.bridge_fee, Some(10_000));
    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["from"]["chain_id"], 42161);
    assert_eq!(body["to"]["kind"], "solana");
    assert_eq!(body["cctp_recipient_setup"], true);
}

#[tokio::test]
async fn layerzero_fee_is_added_to_the_network_fee_on_both_pinned_sides() {
    const PLASMA_USDT0: &str = "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb";
    let layerzero = json!({"router": "layerzero", "native_fee_sats": "450"});
    // As a server released before the fee was charged sends the leg.
    let unpriced = json!({"router": "layerzero"});
    for (chain, bridge_rate, network_fee) in [
        ("9745", Some(layerzero), 500),
        ("9745", Some(unpriced), 50),
        ("42161", None, 50),
    ] {
        for amount in [
            QuoteAmount::Source(100_000),
            QuoteAmount::Target(100_000_000),
        ] {
            let server = MockServer::start().await;
            // As the server sends an OFT leg: no `bridge_fee`, since nothing is deducted
            // from the bridged USDT0.
            let mut dex = json!({"expected_amount_in":{"raw":"1000000000000000","decimals":18},"estimated_amount_out":{"raw":"100000000","decimals":6},"estimated_settlement_seconds":60,"router":"uniswap_v3","cache_ttl_seconds":30});
            if let Some(rate) = &bridge_rate {
                dex["bridge_rate"] = rate.clone();
            }
            Mock::given(path("/dex-quote"))
                .respond_with(ResponseTemplate::new(200).set_body_json(dex))
                .mount(&server)
                .await;
            let (pairs, mut fees, config) = data();
            fees.pairs[0].fees.source_sats = 50;
            let q = Client::new(&server.uri())
                .unwrap()
                .compose_quote_with_data(
                    request(
                        true,
                        Chain::from_wire_str(chain),
                        PLASMA_USDT0,
                        amount.clone(),
                    ),
                    100,
                    &pairs,
                    &fees,
                    &config,
                    0.0,
                )
                .await
                .unwrap();
            assert_eq!(q.network_fee, network_fee, "{chain} {amount:?}");
            assert_eq!(q.bridge_fee, None);
            let fees = u128::from(network_fee);
            match amount {
                QuoteAmount::Source(_) => assert_eq!(
                    q.net_target_amount,
                    ((100_000 - fees) * 100_000_000 / 100_000).to_string()
                ),
                QuoteAmount::Target(_) => {
                    assert_eq!(q.net_source_amount, (100_000 + fees).to_string())
                }
            }
        }
    }
}

#[tokio::test]
async fn large_amount_intermediates_do_not_overflow_or_use_floating_point() {
    let client = Client::new("http://localhost:1").unwrap();
    let (pairs, fees, config) = data();
    let q = client
        .compose_quote_with_data(
            request(
                false,
                Chain::arbitrum(),
                TBTC,
                QuoteAmount::Source(u128::MAX),
            ),
            100,
            &pairs,
            &fees,
            &config,
            0.0,
        )
        .await
        .unwrap();
    assert_eq!(q.source_amount, u128::MAX.to_string());
    assert_eq!(q.target_amount, (u128::MAX / 10_000_000_000).to_string());
}

#[tokio::test]
async fn static_data_is_shared_but_live_fees_are_not_cached() {
    let server = MockServer::start().await;
    let (pairs, fees, config) = data();
    Mock::given(path("/swap-pairs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pairs))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/chain-config"))
        .respond_with(ResponseTemplate::new(200).set_body_json(config))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/network-fees"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fees))
        .expect(2)
        .mount(&server)
        .await;
    let client = Client::new(&server.uri()).unwrap();
    let request = request(true, Chain::arbitrum(), TBTC, QuoteAmount::Source(100_000));
    let (a, b) = tokio::join!(
        client.compose_quote(request.clone(), 100),
        client.compose_quote(request, 100)
    );
    assert_eq!(a.unwrap(), b.unwrap());
}

#[tokio::test]
async fn all_dex_directions_encode_the_correct_pinned_token_amount() {
    for btc_source in [false, true] {
        for amount in [QuoteAmount::Source(100_000), QuoteAmount::Target(100_000)] {
            let server = MockServer::start().await;
            Mock::given(path("/dex-quote")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"expected_amount_in":{"raw":"1000000000000000","decimals":18},"estimated_amount_out":{"raw":"2000000000000000","decimals":18},"estimated_settlement_seconds":0,"router":"uniswap","cache_ttl_seconds":30}))).mount(&server).await;
            let (pairs, fees, config) = data();
            Client::new(&server.uri())
                .unwrap()
                .compose_quote_with_data(
                    request(btc_source, Chain::arbitrum(), USDC, amount.clone()),
                    100,
                    &pairs,
                    &fees,
                    &config,
                    0.0,
                )
                .await
                .unwrap();
            let requests = server.received_requests().await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
            let source_pinned = matches!(amount, QuoteAmount::Source(_));
            assert_eq!(
                body["amount"]["kind"],
                if source_pinned {
                    "exact_in"
                } else {
                    "exact_out"
                }
            );
            assert_eq!(
                body["amount"]["value"],
                if btc_source == source_pinned {
                    "1000000000000000"
                } else {
                    "100000"
                }
            );
            assert_eq!(
                body["from"]["address"],
                if btc_source { TBTC } else { USDC }
            );
        }
    }
}

#[tokio::test]
async fn malformed_rates_and_unsupported_paths_return_errors() {
    let client = Client::new("http://localhost:1").unwrap();
    let (pairs, fees, mut config) = data();
    for rate in ["0", "NaN", "-1", "1.2.3"] {
        config.chains[0].btc_peg_rate = rate.into();
        assert!(
            client
                .compose_quote_with_data(
                    request(true, Chain::arbitrum(), TBTC, QuoteAmount::Source(100_000)),
                    100,
                    &pairs,
                    &fees,
                    &config,
                    0.0
                )
                .await
                .is_err()
        );
    }
    config.chains[0].btc_peg_rate = "1".into();
    assert!(
        client
            .compose_quote_with_data(
                request(true, Chain::arbitrum(), TBTC, QuoteAmount::Source(100_000)),
                100,
                &pairs,
                &fees,
                &config,
                f64::NAN
            )
            .await
            .is_err()
    );
}

/// A partner key's `base_fee_rate` replaces the pair fee; `extra_fee_rate` still adds to it.
#[tokio::test]
async fn partner_base_fee_rate_replaces_pair_fee() {
    let server = MockServer::start().await;
    let client = Client::new(&server.uri()).unwrap();
    let (mut pairs, fees, config) = data();
    for pair in &mut pairs.pairs {
        pair.fee_percentage = 0.005;
    }
    let quote = |referral: ReferralFeeResponse| {
        client.compose_quote_with_data(
            request(true, Chain::arbitrum(), TBTC, QuoteAmount::Source(100_000)),
            100,
            &pairs,
            &fees,
            &config,
            referral,
        )
    };
    let route = quote(ReferralFeeResponse {
        extra_fee_rate: 0.0,
        base_fee_rate: None,
        referral_label: None,
    })
    .await
    .unwrap();
    assert_eq!(route.protocol_fee, 500);
    let partner = quote(ReferralFeeResponse {
        extra_fee_rate: 0.001,
        base_fee_rate: Some(0.002),
        referral_label: Some("partner".into()),
    })
    .await
    .unwrap();
    assert_eq!(
        partner.protocol_fee, 300,
        "0.2% base + 0.1% extra on 100k sats"
    );
    assert!(
        (partner.protocol_fee_rate - 0.003).abs() < 1e-12,
        "{}",
        partner.protocol_fee_rate
    );
}
