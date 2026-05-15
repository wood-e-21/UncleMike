#!/usr/bin/env bash
# Enforce anti-pattern #4: SQL must only appear in backend/src/db/.
#
# How it works:
#   - Find every file outside backend/src/db/ that contains `sqlx::query`.
#   - Compare against the "legacy whitelist" — files we KNOW still have
#     inline SQL because they predate the db::repositories migration.
#   - Any file that's in the offender list but NOT on the whitelist is
#     a regression. CI fails.
#   - When you migrate a file off inline SQL, remove it from the
#     whitelist below. The lint then guards it from regressions.
#
# Intentionally simple: bash + grep, no extra build deps.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Files where inline sqlx is currently tolerated. SHRINKING this list
# is how we make progress on anti-pattern #4. Adding to this list
# requires a justification in your PR description.
LEGACY_WHITELIST=(
    # routes/matters.rs and routes/clients.rs were migrated to
    # db::repositories in the Phase B3 cleanup. DO NOT add them back.
    "backend/src/routes/chat.rs"
    "backend/src/routes/documents.rs"
    "backend/src/routes/projects.rs"
    "backend/src/routes/sync.rs"
    "backend/src/routes/tabular_reviews.rs"
    "backend/src/routes/user.rs"
    "backend/src/routes/workflows.rs"
    "backend/src/auth/session.rs"
    "backend/src/embeddings/service.rs"
    "backend/src/embeddings/chunker.rs"
    "backend/src/sync/scanner.rs"
    "backend/src/llm/builtin_tools.rs"
    "backend/src/mikeprj/io.rs"
    # lib.rs has the recovery `UPDATE documents SET status = 'interrupted'`
    # at startup — could move into db::startup_recovery in a follow-up.
    "backend/src/lib.rs"
)

# Find every file outside backend/src/db/ that mentions sqlx::query.
# Use a tmpfile instead of `mapfile` so this works on macOS bash 3.2.
TMP_FOUND="$(mktemp)"
trap 'rm -f "$TMP_FOUND"' EXIT
grep -rln 'sqlx::query' "$ROOT/backend/src" \
    --exclude-dir=db \
    2>/dev/null | sed "s|^$ROOT/||" | sort -u >"$TMP_FOUND"

violators=""
while IFS= read -r path; do
    [ -z "$path" ] && continue
    skip=0
    for allowed in "${LEGACY_WHITELIST[@]}"; do
        if [ "$path" = "$allowed" ]; then
            skip=1
            break
        fi
    done
    if [ "$skip" -eq 0 ]; then
        violators="${violators}${path}\n"
    fi
done <"$TMP_FOUND"

if [ -n "$violators" ]; then
    echo "ERROR: inline sqlx::query found in files NOT on the legacy whitelist:" >&2
    printf "%b" "$violators" | sed 's/^/  - /' >&2
    echo >&2
    echo "Anti-pattern #4 (docs/00-anti-patterns.md) forbids SQL outside" >&2
    echo "backend/src/db/. Either:" >&2
    echo "  (a) move the query to backend/src/db/repositories/<name>.rs, OR" >&2
    echo "  (b) if this is a temporary inline SQL during a larger migration," >&2
    echo "      add the file to LEGACY_WHITELIST in scripts/lint-db-isolation.sh" >&2
    echo "      AND explain it in your PR description." >&2
    exit 1
fi

echo "OK — no inline sqlx outside backend/src/db/ except on the legacy whitelist."
echo "Whitelist size: ${#LEGACY_WHITELIST[@]} (shrink this over time)."
