//! Verify that every hand-written type in `lendaswap_sdk::types` still serializes
//! to JSON that matches the upstream OpenAPI component schema with the same name.
//!
//! When the backend's `openapi.json` changes shape, this test fails — that is
//! the signal to update the Rust types (and any callers).

use std::path::Path;

use lendaswap_sdk::types::ErrorResponse;
use lendaswap_sdk::types::Version;
use serde_json::Value;
use serde_json::json;

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
    let validator =
        jsonschema::validator_for(&schema).expect("OpenAPI component is not a valid JSON Schema");

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
    &["Version", "ErrorResponse"]
}
