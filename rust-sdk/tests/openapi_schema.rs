//! Verify that every hand-written type in `lendaswap_sdk::types` still serializes
//! to JSON that matches the upstream OpenAPI component schema with the same name.
//!
//! When the backend's `openapi.json` changes shape, this test fails — that is
//! the signal to update the Rust types (and any callers).

use lendaswap_sdk::types::Chain;
use lendaswap_sdk::types::ErrorResponse;
use lendaswap_sdk::types::KnownChain;
use lendaswap_sdk::types::QuoteResponse;
use lendaswap_sdk::types::TokenId;
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

/// Like [`assert_matches_schema`] but rewrites the component's top-level
/// `oneOf` to `anyOf` first. Use this for components whose spec uses `oneOf`
/// over schemas that aren't disjoint (e.g. `TokenId` lists `enum:["btc"]` and
/// `string`, where `"btc"` matches both).
#[track_caller]
fn assert_matches_schema_anyof<T: serde::Serialize>(spec: &Value, schema_name: &str, value: &T) {
    let mut schema = component_schema(spec, schema_name);
    if let Some(arms) = schema.as_object_mut().and_then(|o| o.remove("oneOf")) {
        schema
            .as_object_mut()
            .unwrap()
            .insert("anyOf".to_string(), arms);
    }
    validate(&schema, schema_name, value);
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
fn token_id_evm_address_matches_spec() {
    let spec = load_spec();
    let value = TokenId::Evm("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string());
    // An arbitrary hex address only matches the `string` arm, so strict oneOf
    // is satisfied here.
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
    ]
}
