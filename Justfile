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

typecheck:
    cd ts-pure-sdk && pnpm run typecheck

# Bump version for the SDK
bump-version version:
    cd ts-pure-sdk && npm version {{ version }} --no-git-tag-version

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
