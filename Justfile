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
    cd ts-pure-sdk && npm run generate:api

build:
    cd ts-pure-sdk && npm install && npm run build

typecheck:
    cd ts-pure-sdk && npm run typecheck

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
# Format Rust sources only.
fmt-rust:
    cd rust-sdk && cargo +nightly-2025-11-01 fmt

# Format / auto-fix TypeScript sources only (via biome).
fmt-ts:
    cd ts-pure-sdk && npm run lint:fix

# FIXME: ts-pure-sdk tests are broken (better-sqlite3 native binding fails to
# load against the current Node ABI). Re-add a `test-ts` dependency here
# once that's fixed.
#
# Run unit tests across SDKs.
test: test-rust

# Run Rust unit tests only.
test-rust:
    cd rust-sdk && cargo test

# Lint: clippy for Rust (deny warnings) + biome for TypeScript.
lint: lint-rust lint-ts

# Lint Rust only (no node required).
lint-rust:
    cd rust-sdk && cargo clippy --all-targets -- -D warnings

# Lint TypeScript only (biome).
lint-ts:
    cd ts-pure-sdk && npm run lint
