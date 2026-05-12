# docs/04-versioning.md

> Schema and version compatibility rules across SQLite, frontmatter, sidecars, and the workspace itself.

## Four independent version axes

| Axis | What it versions | Where it's stored |
|---|---|---|
| **Workspace schema** | The on-disk layout (`docs/01-workspace-layout.md`) | `<workspace>/.mike/workspace.json::schema_version` |
| **SQLite schema** | The `.db` file structure | `_migrations` table |
| **Frontmatter schema** | A single `.md` file's frontmatter shape | `<item>.frontmatter.schema_version` |
| **Sidecar API** | A sidecar's HTTP request/response shape | sidecar `/version` `.schema_version` |

These evolve independently. Backend code declares the version it supports on each axis.

## Mike binary version

`Cargo.toml` and the root `package.json` carry the same semver. Bumped on every release. This is the user-visible "Mike 0.3.1" number.

## Workspace schema versioning

```json
{
  "schema_version": 1,
  "id": "01J...",
  "created_at": "2026-05-08T...",
  "mike_version_first": "0.1.0"
}
```

On opening a workspace:
- `schema_version <= supported`: open normally; run any pending workspace-level migrations (e.g., move folders around).
- `schema_version > supported`: refuse to open. Error: "This workspace was last opened by a newer Mike (schema v3). Update Mike or pick a different workspace."

Workspace-level migrations are rare; reserved for changes that touch directory layout (e.g., introducing the `corpora/` directory).

## SQLite schema versioning

`sqlx` migrations live in `backend/migrations/` named `NNNN_description.sql`. Applied in lexical order; recorded in `_migrations` table.

Rules:
1. **Migrations are append-only.** Never edit a committed migration.
2. **Forward-only by default.** Reversibility is nice-to-have, not required.
3. **One semantic change per migration.** A migration that "adds matters tables and changes chunks_vec partition keys" is two migrations.
4. **Migrations are idempotent** — running twice produces the same result.
5. **Schema-only.** Data migrations (e.g., re-chunking everything) are jobs in the queue, not migration scripts.

On startup:
- Check `PRAGMA user_version` against the highest migration filename's number.
- Run pending migrations in a single transaction.
- If a migration fails, abort startup and surface a clear error.

The migration runner refuses to start if it sees `_migrations` entries newer than the highest file on disk (means the DB was last opened by a newer Mike).

## Frontmatter schema versioning

Each `.md` file declares its own `schema_version`. Rules:

- **Adding optional fields:** no version bump. Old items lack the field; readers treat absence as default.
- **Adding required fields:** version bump. Old-version items get the new field populated by a migration function at read time.
- **Renaming or restructuring fields:** version bump. Migration rewrites the file on read, atomically (`.tmp` + rename).
- **Removing fields:** drop on the next version bump; readers tolerate stray fields.

Migration functions live at `backend/src/storage/migrations/frontmatter_v{n}_to_v{n+1}.rs`. Each is a pure `fn migrate(input: serde_yaml::Value) -> serde_yaml::Value` plus a body transformer if needed.

Reading flow:
```rust
let (fm_raw, body) = read_md(path)?;
let version: u32 = fm_raw["schema_version"].as_u64().unwrap_or(0) as u32;
let (fm_raw, body) = if version < CURRENT {
    let migrated = run_migrations(version, fm_raw, body);
    write_md_atomic(path, &migrated.0, &migrated.1)?;   // upgrade in place
    migrated
} else if version > CURRENT {
    return Err(NewerSchemaError { path, version });
} else {
    (fm_raw, body)
};
let fm: Frontmatter = serde_yaml::from_value(fm_raw)?;
```

Items with `schema_version > supported` are not silently ignored — they appear in the UI as "needs newer Mike" and exclude from search.

Golden tests in `tests/fixtures/frontmatter/v{n}/` verify each migration produces the expected v{n+1} output.

## Sidecar API versioning

Two version numbers per sidecar:

- **`version`**: code version of the sidecar binary (e.g. `1.2.3`). Cosmetic; for logs and diagnostics.
- **`schema_version`**: integer. Bumped only on **breaking changes** to request/response shape.

Backend declares `expected_schema_version()` per sidecar. On spawn:
- Match: proceed.
- Sidecar's `schema_version < expected`: refuse, `degraded` state.
- Sidecar's `schema_version > expected`: refuse, `degraded` state.

Sidecars cannot be silently upgraded across schema versions — would need a Mike update first. The frontend banner says "Sidecar 'docling' requires Mike >= 0.4.0; you're on 0.3.0."

## Audit log version

`<workspace>/.mike/logs/audit.log` lines have their own format version:

```json
{"v":1, "ts":"2026-05-08T...", "event":"auth.login", "actor":"local-user", ...}
```

If we ever change the format, increment `v`. Old lines remain readable.

## Compatibility matrix policy

For each Mike release, the changelog explicitly states:

```
Compatibility (Mike 0.4.0):
  workspace schema:   1 (no change)
  sqlite schema:      ≤17 (up from 14; runs 5 migrations automatically)
  frontmatter:        ≤2 (upgrades v1 in place on first read)
  sidecar docling:    schema 1 (no change), version ≥1.1.0
  sidecar eyecite:    schema 2 (BREAKING), version ≥1.0.0
                      ⚠️ Requires updating bundled eyecite binary
```

Users who skip versions are supported up to N-2 (you can update directly from 0.2.0 to 0.4.0, not from 0.1.0 to 0.4.0 — would need 0.1 → 0.2 → 0.4).

## Breaking changes policy

A breaking change is anything that requires a migration or version bump on the axes above. Each breaking change:
1. Documented in the changelog with rationale.
2. Has a migration script or function.
3. Has a golden test verifying the migration.
4. Bumps the relevant `schema_version`.

Breaking changes accumulate in `main` between releases. The release itself runs migrations on first launch.
