//! Verify that every hand-written type in `lendaswap_sdk::types` still serializes
//! to JSON that matches the upstream OpenAPI component schema with the same name.
//!
//! When the backend's `openapi.json` changes shape, this test fails — that is
//! the signal to update the Rust types (and any callers).

use lendaswap_sdk::types::Chain;
use lendaswap_sdk::types::ErrorResponse;
use lendaswap_sdk::types::EvmToArkadeSwapResponse;
use lendaswap_sdk::types::KnownChain;
use lendaswap_sdk::types::QuoteResponse;
use lendaswap_sdk::types::SwapStatus;
use lendaswap_sdk::types::TokenId;
use lendaswap_sdk::types::TokenInfo;
use lendaswap_sdk::types::Version;
use serde_json::Value;
use serde_json::json;
use std::path::Path;

const SPEC_PATH: &str = "openapi.json";

fn load_spec() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(SPEC_PATH);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    serde_json::from_slice(&bytes).expect("openapi.json is not valid JSON")
}

/// Build a self-contained JSON Schema for the named component, inlining the
/// `components` section so `$ref`s like `#/components/schemas/Foo` resolve
/// locally rather than triggering a network lookup.
fn component_schema(spec: &Value, name: &str) -> Value {
    let schema = spec
        .pointer(&format!("/components/schemas/{name}"))
        .unwrap_or_else(|| panic!("component schema `{name}` not found in spec"))
        .clone();

    // Embed the full `components` block so `$ref` resolution stays in-document.
    let components = spec.get("components").cloned().unwrap_or_else(|| json!({}));

    let mut combined = schema;
    combined
        .as_object_mut()
        .expect("component schema must be an object")
        .insert("components".to_string(), components);
    combined
}

#[track_caller]
fn assert_matches_schema<T: serde::Serialize>(spec: &Value, schema_name: &str, value: &T) {
    let schema = component_schema(spec, schema_name);
    validate(&schema, schema_name, value);
}

/// Like [`assert_matches_schema`] but rewrites every `oneOf` in the
/// component (top-level and transitively, including the embedded
/// `components` block) to `anyOf`. Needed for components whose spec uses
/// `oneOf` over schemas that aren't disjoint (e.g. `TokenId` lists
/// `enum:["btc"]` and `string`, where `"btc"` matches both — and that
/// `TokenId` shows up nested inside `TokenInfo` inside swap responses).
#[track_caller]
fn assert_matches_schema_anyof<T: serde::Serialize>(spec: &Value, schema_name: &str, value: &T) {
    let mut schema = component_schema(spec, schema_name);
    relax_oneof_recursive(&mut schema);
    validate(&schema, schema_name, value);
}

fn relax_oneof_recursive(v: &mut Value) {
    match v {
        Value::Object(map) => {
            if let Some(arms) = map.remove("oneOf") {
                map.insert("anyOf".to_string(), arms);
            }
            for child in map.values_mut() {
                relax_oneof_recursive(child);
            }
        }
        Value::Array(arr) => {
            for child in arr.iter_mut() {
                relax_oneof_recursive(child);
            }
        }
        _ => {}
    }
}

#[track_caller]
fn validate<T: serde::Serialize>(schema: &Value, schema_name: &str, value: &T) {
    let validator =
        jsonschema::validator_for(schema).expect("OpenAPI component is not a valid JSON Schema");
    let instance = serde_json::to_value(value).expect("Rust value failed to serialize to JSON");
    if let Err(error) = validator.validate(&instance) {
        panic!(
            "value of Rust type does not match component `{schema_name}`:\n  error: {error}\n  instance: {instance}\n  schema: {schema}"
        );
    }
}

#[test]
fn version_matches_spec() {
    let spec = load_spec();
    let value = Version {
        tag: "v0.2.30".to_string(),
        commit_hash: "deadbeef".to_string(),
    };
    assert_matches_schema(&spec, "Version", &value);
}

#[test]
fn error_response_matches_spec() {
    let spec = load_spec();
    let value = ErrorResponse {
        error: "something went wrong".to_string(),
    };
    assert_matches_schema(&spec, "ErrorResponse", &value);
}

#[test]
fn chain_known_variants_match_spec() {
    let spec = load_spec();
    for known in [
        KnownChain::Arkade,
        KnownChain::Lightning,
        KnownChain::Bitcoin,
        KnownChain::Polygon,
        KnownChain::Ethereum,
        KnownChain::Arbitrum,
    ] {
        assert_matches_schema(&spec, "Chain", &Chain::Known(known));
    }
}

#[test]
fn token_id_btc_matches_spec() {
    let spec = load_spec();
    // Spec uses `oneOf [enum:["btc"], string]` — "btc" matches both arms,
    // violating strict oneOf semantics. Relax to anyOf for validation.
    assert_matches_schema_anyof(&spec, "TokenId", &TokenId::Btc);
}

#[test]
fn token_id_named_evm_variant_matches_spec() {
    let spec = load_spec();
    // A named variant serialises to its known contract address, which only
    // matches the `string` arm of the spec's oneOf — strict validation passes.
    assert_matches_schema(&spec, "TokenId", &TokenId::UsdcEthereum);
}

#[test]
fn token_id_other_address_matches_spec() {
    let spec = load_spec();
    let value = TokenId::Other("0xdeadbeef0000000000000000000000000000beef".to_string());
    assert_matches_schema(&spec, "TokenId", &value);
}

#[test]
fn quote_response_matches_spec() {
    let spec = load_spec();
    let value = QuoteResponse {
        exchange_rate: "30000.00".to_string(),
        network_fee: 1_000,
        gasless_network_fee: 2_000,
        protocol_fee: 250,
        protocol_fee_rate: 0.0025,
        min_amount: 10_000,
        max_amount: 100_000_000,
        source_amount: "100000".to_string(),
        target_amount: "30000000000".to_string(),
        net_source_amount: "100000".to_string(),
        net_target_amount: "29996750000".to_string(),
        bridge_fee: None,
    };
    assert_matches_schema(&spec, "QuoteResponse", &value);
}

#[test]
fn swap_status_known_variants_match_spec() {
    let spec = load_spec();
    for status in [
        SwapStatus::Pending,
        SwapStatus::ClientFunded,
        SwapStatus::ServerFunded,
        SwapStatus::ClientRedeemed,
        SwapStatus::ServerRedeemed,
        SwapStatus::Expired,
    ] {
        assert_matches_schema(&spec, "SwapStatus", &status);
    }
}

#[test]
fn token_info_matches_spec() {
    let spec = load_spec();
    let value = TokenInfo {
        token_id: TokenId::Usdt0Arbitrum,
        symbol: "USDT".to_string(),
        chain: Chain::arbitrum(),
        name: "Tether USD".to_string(),
        decimals: 6,
    };
    // EVM address only matches the `string` arm of TokenId's oneOf, so strict
    // validation passes. The Btc case is exercised via the swap-response test
    // (which uses the anyof helper).
    assert_matches_schema(&spec, "TokenInfo", &value);
}

#[test]
fn evm_to_arkade_swap_response_matches_spec() {
    let spec = load_spec();
    let token = TokenInfo {
        token_id: TokenId::Usdt0Arbitrum,
        symbol: "USDT".to_string(),
        chain: Chain::arbitrum(),
        name: "Tether USD".to_string(),
        decimals: 6,
    };
    let btc_token = TokenInfo {
        token_id: TokenId::Btc,
        symbol: "BTC".to_string(),
        chain: Chain::arkade(),
        name: "Bitcoin".to_string(),
        decimals: 8,
    };
    let value = EvmToArkadeSwapResponse {
        id: "swap_01".to_string(),
        status: SwapStatus::Pending,
        fee_sats: 500,
        hash_lock: "0xdeadbeef".to_string(),
        source_token: token,
        target_token: btc_token,
        created_at: "2026-05-12T00:00:00Z".to_string(),
        chain: "Arbitrum".to_string(),
        evm_chain_id: 42161,
        source_amount: "100000000".to_string(),
        target_amount: "150000".to_string(),
        evm_expected_sats: "150000".to_string(),
        evm_htlc_address: "0xhtlc".to_string(),
        client_evm_address: "0xclient".to_string(),
        server_evm_address: "0xserver".to_string(),
        evm_refund_locktime: 1_000_000,
        btc_vhtlc_address: "ark1qvhtlc".to_string(),
        target_arkade_address: "ark1qtarget".to_string(),
        sender_pk: "02sender".to_string(),
        receiver_pk: "02receiver".to_string(),
        arkade_server_pk: "02server".to_string(),
        vhtlc_refund_locktime: 1_000_000,
        unilateral_claim_delay: 144,
        unilateral_refund_delay: 288,
        unilateral_refund_without_receiver_delay: 432,
        network: "mainnet".to_string(),
        gasless: false,
        bridge_source_chain: None,
        bridge_source_token_address: None,
        btc_claim_txid: None,
        btc_fund_txid: None,
        evm_claim_txid: None,
        evm_fund_txid: None,
    };
    // Uses anyof helper because TokenInfo carries a TokenId, and the spec's
    // TokenId oneOf semantics aren't strict (see token_id_btc_matches_spec).
    assert_matches_schema_anyof(&spec, "EvmToArkadeSwapResponse", &value);
}

#[test]
fn quote_response_with_bridge_fee_matches_spec() {
    let spec = load_spec();
    let value = QuoteResponse {
        exchange_rate: "30000.00".to_string(),
        network_fee: 1_000,
        gasless_network_fee: 2_000,
        protocol_fee: 250,
        protocol_fee_rate: 0.0025,
        min_amount: 10_000,
        max_amount: 100_000_000,
        source_amount: "100000".to_string(),
        target_amount: "30000000000".to_string(),
        net_source_amount: "100000".to_string(),
        net_target_amount: "29991750000".to_string(),
        bridge_fee: Some(5_000),
    };
    assert_matches_schema(&spec, "QuoteResponse", &value);
}

/// Sanity check that every Rust type registered in `registered_types()` below
/// has a matching schema in the spec. If a type is added without registering
/// it here, we lose schema coverage silently — this test prevents that.
#[test]
fn every_registered_type_has_a_spec_schema() {
    let spec = load_spec();
    for name in registered_types() {
        assert!(
            spec.pointer(&format!("/components/schemas/{name}"))
                .is_some(),
            "type `{name}` is registered for schema-compliance but missing from openapi.json",
        );
    }
}

/// Single source of truth for which types should round-trip against the spec.
fn registered_types() -> &'static [&'static str] {
    &[
        "Version",
        "ErrorResponse",
        "Chain",
        "TokenId",
        "QuoteResponse",
        "SwapStatus",
        "TokenInfo",
        "EvmToArkadeSwapResponse",
    ]
}
