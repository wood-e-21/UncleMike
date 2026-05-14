dev:
    npm run dev

build:
    npm run build

test:
    cargo test -p mike-backend --no-default-features --features local-storage

typecheck:
    npm run build:electron

codegen:
    echo "codegen not yet implemented, see PLAN.md"

package:
    npm run dist:dir

# Enforce anti-pattern #4: SQL must live in backend/src/db/*. Once a
# route file has been refactored to call db::repositories, add it to
# the excluded directories below by removing it from this whitelist.
#
# Today the whitelist of "files we accept inline SQL in" is broad —
# legacy MikeRust handlers still inline sqlx. The list shrinks every
# time a route is migrated. CI fails if SQL appears in a file NOT on
# this list, which prevents new violations.
#
# To check which legacy files still need migration:
#   just lint-db-isolation-status
lint-db-isolation:
    @echo "Checking SQL is constrained to db/ (and explicitly-allowed legacy files)..."
    @./scripts/lint-db-isolation.sh

lint-db-isolation-status:
    @echo "Files still containing inline sqlx queries outside backend/src/db/:"
    @grep -rln 'sqlx::query' backend/src/routes backend/src/llm backend/src/sync backend/src/embeddings backend/src/mcp 2>/dev/null | sort || true

# Run all lints. Add to CI.
lint: lint-db-isolation
