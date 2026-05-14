//! Repository layer — every SQL query lives here.
//!
//! Anti-pattern #4 (docs/00-anti-patterns.md): SQL must not appear in
//! `routes/`, `llm/`, `sync/`, etc. Route handlers call into this
//! module, which returns typed structs from `db::models`. The lint at
//! `scripts/lint-db-isolation.sh` is enforced in CI; the legacy
//! whitelist there shrinks every time a route is migrated.
//!
//! Why "repositories" and not "queries"? The name signals the long-term
//! direction (Phase 4 + 7 in PLAN.md): each module owns one domain
//! aggregate (matters + their items + attachments) including any
//! invariants spanning multiple tables. The `matters` repository is
//! the only place that knows how to keep `matters.matter.md` in sync
//! with the `matters` table.

pub mod clients;
pub mod matters;
