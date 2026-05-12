# Use shell positional args ($1, $2, "$@", …) so variadic recipes can
# forward args without losing quoting (e.g. `--source-amount "10 USD"`
# stays one token across forwarding hops).
set positional-arguments := true

# =============================================================================
# Database (SQLx migrations for core)
# =============================================================================
# Default database path for development/testing

DB_PATH := './lendaswap-client.db'
DB_URL := 'sqlite:' + DB_PATH

# Create the database file if it doesn't exist
db-create:
    #!/usr/bin/env bash
    cd core
    if [ ! -f "{{ DB_PATH }}" ]; then
        echo "Creating database at {{ DB_PATH }}..."
        mkdir -p "$(dirname "{{ DB_PATH }}")"
        touch "{{ DB_PATH }}"
        echo "Database created."
    else
        echo "Database already exists at {{ DB_PATH }}"
    fi

# Prepare SQLx offline query data (run after changing queries)

# Note: doesn't need db-create since it only analyzes queries, doesn't connect to DB
db-prepare: db-create
    cd core && cargo sqlx prepare

# Add a new migration (creates up/down SQL files)
db-add-migration name:
    sqlx migrate add --source ./core/migrations -r {{ name }}

# Run pending migrations
db-run-migration: db-create
    sqlx migrate run --source ./core/migrations --database-url={{ DB_URL }}

# Revert the last migration
db-revert-migration:
    sqlx migrate revert --source ./core/migrations --database-url={{ DB_URL }}

# Show migration status
db-status:
    sqlx migrate info --source ./core/migrations --database-url={{ DB_URL }}

# Run SQLite tests with a file-based database (includes date and commit hash in filename)
test-sqlite:
    #!/usr/bin/env bash
    DATE=$(date +%Y-%m-%d)
    COMMIT=$(git rev-parse --short HEAD)
    DB_FILE="./lendaswap-test_${DATE}_${COMMIT}.db"
    echo "Running SQLite tests with database: $DB_FILE"
    TEST_SQLITE_DB_PATH="$DB_FILE" cargo test -p lendaswap-core --features sqlite storage:: -- --test-threads=1

# =============================================================================
# Pure TypeScript SDK
# =============================================================================

generate:
    cd ts-pure-sdk && pnpm run generate:api

build:
    cd ts-pure-sdk && pnpm install && pnpm run build
    cd ts-sdk && pnpm install && pnpm run build

typecheck:
    cd ts-pure-sdk && pnpm run typecheck

# Bump version for the SDK
bump-version version:
    cd ts-pure-sdk && npm version {{ version }} --no-git-tag-version

# =============================================================================
# Cross-SDK developer recipes
#
# `fmt` / `test` / `lint` cover every SDK in this directory and are the
# natural local commands. The per-language `*-rust` / `*-ts` sub-recipes
# exist so CI (and other callers) can invoke just one half without pulling
# in dependencies for the other (e.g. running Rust clippy without
# installing node + biome).
#
# The root `justfile` delegates to the rust-only sub-recipes from `clippy`
# and `test-rust` so those CI jobs stay lean; local devs typically want
# `just client-sdk lint` / `just client-sdk test` / `just client-sdk fmt`.
# =============================================================================

# Format Rust + TypeScript sources.
fmt: fmt-rust fmt-ts

# `rustfmt.toml` uses unstable options, so we invoke the same pinned nightly
# that `dprint`'s exec plugin uses (see `scripts/rustfmt-nightly.sh`).
#
# Format Rust sources only. Covers every Rust crate under client-sdk:
# rust-sdk (the pure-Rust SDK) AND dotnet-sdk/native (the FFI shim).
fmt-rust:
    cd rust-sdk && cargo +nightly-2025-11-01 fmt
    cd dotnet-sdk/native && cargo +nightly-2025-11-01 fmt

# Format / auto-fix TypeScript sources only (via biome).
fmt-ts:
    cd ts-pure-sdk && npm run lint:fix

# FIXME: ts-pure-sdk tests are broken (better-sqlite3 native binding fails to
# load against the current Node ABI). Re-add a `test-ts` dependency here
# once that's fixed. dotnet-sdk tests live under `test-dotnet` because
# they need the `dotnet` CLI installed — not everyone has it.
#
# Run Rust unit tests (currently the only working set).
test: test-rust

# Run Rust unit tests only (rust-sdk; dotnet-sdk/native is pure FFI scaffolding).
test-rust:
    cd rust-sdk && cargo test

# Lint: clippy for Rust + biome for TypeScript (C# build excluded — see lint-dotnet).
lint: lint-rust lint-ts

# Lint Rust only (no node required). Covers rust-sdk + dotnet-sdk/native.
lint-rust:
    cd rust-sdk && cargo clippy --all-targets -- -D warnings
    cd dotnet-sdk/native && cargo clippy --all-targets -- -D warnings

# Lint TypeScript only (biome).
lint-ts:
    cd ts-pure-sdk && npm run lint

# =============================================================================
# dotnet-sdk recipes (require the .NET SDK + uniffi-bindgen-cs).
#
# These are gated behind explicit recipe names because they pull in a
# separate toolchain. Local devs without `dotnet` can ignore them; CI
# runs them in a dedicated job that sets up dotnet on the runner.
# =============================================================================

# Build the C# solution (also rebuilds the native cdylib + regenerates bindings).
build-dotnet:
    cd dotnet-sdk && just build

# Run the C# test suite.
test-dotnet:
    cd dotnet-sdk && just test

# Lint dotnet-sdk: clippy on the native FFI crate (C# side has no linter wired yet).
lint-dotnet:
    cd dotnet-sdk && just lint-rust

# Run the .NET sample CLI. Pass arguments after `--`, e.g.:
#   just client-sdk dotnet-cli -- quote --source Arb:USDT --target Arkade:BTC --source-amount "10 USD"
dotnet-cli *args:
    cd dotnet-sdk && just dotnet-cli "$@"

# =============================================================================
# Changesets (SDK release versioning + changelogs)
# =============================================================================
# Usage (commit the generated .changeset/*.md alongside your change):
#   just changeset pure                 # @lendasat/lendaswap-sdk-pure
#   just changeset satora               # @satora/*
#   just changeset pure add --empty     # empty changeset (no bump) for tooling
#   just changeset satora status        # any changeset subcommand

# Add a changeset for an SDK workspace (pure | satora), interactive
changeset sdk *args:
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ sdk }}" in
      pure)   dir=ts-pure-sdk ;;
      satora) dir=ts-sdk ;;
      *) echo "unknown sdk '{{ sdk }}' — use: pure | satora"; exit 1 ;;
    esac
    cd "$dir" && pnpm changeset {{ args }}
