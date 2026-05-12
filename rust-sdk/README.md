# lendaswap-sdk

Rust client SDK for the Lendaswap API.

## Design

- **Hand-written types**, not generated. Each request/response struct in `src/types.rs` mirrors a named component schema in `openapi.json`.
- **Schema-compliance tests** (`tests/openapi_schema.rs`) serialize each Rust type to JSON and validate it against the upstream spec. If the backend's schema changes, these tests fail before runtime does.
- **FFI-friendly public API**: no generics or borrowed inputs on public methods, errors are an owned `enum`. The intent is to project this same surface across a future C-ABI shim (e.g. `csbindgen` / `interoptopus`) to ship a NuGet package — no public-API rewrite required.

## Layout

```
rust-sdk/
├── Cargo.toml          # standalone crate (not part of the root workspace)
├── openapi.json        # pinned copy of the API spec, refreshed via `just export-openapi-sdk`
├── src/
│   ├── lib.rs          # public re-exports
│   ├── client.rs       # reqwest-based HTTP client
│   ├── error.rs        # SDK error enum
│   └── types.rs        # request / response types
└── tests/
    ├── openapi_schema.rs   # validates Rust types against openapi.json
    ├── client_mock.rs      # wiremock-based unit tests for the client
    └── integration_live.rs # live tests against a running server (ignored by default)
```

## Develop

```bash
cd client-sdk/rust-sdk

# Build
cargo build

# Run all tests (schema compliance + mock-server unit tests)
cargo test
```

## Live integration tests

`tests/integration_live.rs` hits a real server and is `#[ignore]`d by default. Defaults to `http://localhost:3333`; override with `LENDASWAP_API_URL`.

```bash
# Against a local server
cargo test --test integration_live -- --ignored

# Against another deployment
LENDASWAP_API_URL=https://staging.example.com \
  cargo test --test integration_live -- --ignored
```

## Refresh the OpenAPI spec

From the repo root:

```bash
just export-openapi-sdk
```

This regenerates `ts-pure-sdk/openapi.json` from the live `swap` server and copies it into `rust-sdk/openapi.json`. Re-run `cargo test` — failing schema tests pinpoint which types need updating.

## Adding a new type

1. Add the struct in `src/types.rs` with `#[derive(Serialize, Deserialize)]`. Field names must match the OpenAPI property names exactly.
2. Register it in `registered_types()` in `tests/openapi_schema.rs` and add a `#[test]` that calls `assert_matches_schema(...)` with a representative value.
3. (If wiring an endpoint) add the method on `Client` and a `wiremock` test in `tests/client_mock.rs`.
