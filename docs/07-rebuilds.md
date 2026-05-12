# docs/07-rebuilds.md

> How to derive `<workspace>/.mike/mike.db` from the on-disk `.md` files. The resilience guarantee that makes the Markdown-on-disk design worth its complexity cost.

## Why this exists

SQLite files corrupt. Disks fail mid-write. Power dies. Backup restores create dual writers. Bugs delete rows that shouldn't be deleted.

If `mike.db` is the source of truth, any of these is catastrophic. With this design, `mike.db` is *derivable* — running `mike rebuild-index` recreates it from the `.md` files, which are the actual source of truth.

This document is the contract for that derivation.

## When rebuild runs

### Automatic

On every backend startup:
- **Spot check:** read 10 random items, compare `content_hash` (frontmatter) to SQLite's stored hash. Mismatch → enqueue a `reindex_item` job for the affected item.
- **Missing-DB:** if `mike.db` doesn't exist, prompt for PIN, then full rebuild.
- **Schema-version drift:** if SQLite schema is older than the binary expects, run migrations; if the migration cannot complete cleanly, fall back to full rebuild.

### Manual

`File → Rebuild index from disk` in the frontend. Confirmation dialog. Runs full rebuild against current workspace.

CLI: `mike rebuild-index --workspace <path>` (requires PIN via TTY).

## Algorithm

```
function full_rebuild(workspace):
    require: PIN unlocked (for SQLCipher key)
    require: workspace lock held (no other Mike instances)

    1. Drop existing mike.db if present.
    2. Create new mike.db with SQLCipher, run all migrations.
    3. Walk <workspace>/matters/ in deterministic order:
       a. Process each <client>/client.md → insert into `clients` table.
       b. For each <client>/<matter>/matter.md → insert into `matters`.
       c. For each <client>/<matter>/items/*.md:
            - parse frontmatter, validate against schema
            - insert into `items` table (path, hash, kind, matter_id, ...)
            - enqueue `chunk_and_embed` job (chunks + FTS5 + vec0 happen async)
       d. For each <client>/<matter>/attachments/*:
            - compute sha256
            - insert into `attachments` table with ref_count=0
            - ref_count populated in step 4
       e. (Optionally) <client>/<matter>/chats/*.md → insert chat records.
    4. Compute attachment ref_counts by scanning items[*].attachments.
    5. Validate referential integrity:
        - every item's matter_id resolves
        - every matter's client_id resolves
        - every attachment ref'd by items has a file present
        - missing files → log warnings, do not fail
    6. Drain the embed job queue until empty (or background, see below).
    7. Mark rebuild complete; release lock.
```

## Idempotency

Running rebuild twice produces the same final SQLite state. The walk is deterministic (sort matters by slug, items by ULID). The embed jobs are idempotent (same chunk → same vector via the same model).

## Progress reporting

Rebuild publishes SSE events:
```
{ "type": "rebuild.started", "total_estimated": <N> }
{ "type": "rebuild.progress", "stage": "scanning", "processed": 142, "total": 503 }
{ "type": "rebuild.progress", "stage": "embedding", "processed": 38, "total": 503 }
{ "type": "rebuild.completed", "items": 503, "chunks": 8217, "duration_secs": 87 }
```

Frontend renders a modal with progress bar. User cannot use Mike during rebuild (workspace lock held); UI shows the progress modal exclusively.

## Foreground vs background embedding

For workspaces with > 1000 items:
- **Foreground:** scan + insert items + matters + clients + attachments. Blocking. Must complete before workspace is usable.
- **Background:** chunking, embedding, FTS5 population. Items appear in search incrementally as embeddings land. Mike is usable during this phase; search results are partial until done.

The foreground portion has a hard target (see `docs/09-performance.md`): under 5 minutes for a 10k-item workspace.

## What gets lost on rebuild vs preserved

| Data | Source | Survives rebuild? |
|---|---|---|
| Items, matters, clients | `.md` files | ✅ (rebuild reads them) |
| Item bodies, frontmatter | `.md` files | ✅ |
| Attachments | binary files | ✅ |
| Chunks | derived | ✅ (re-chunked from item bodies) |
| Embeddings | derived | ✅ (re-embedded; identical results) |
| FTS5 index | derived | ✅ (rebuilt from chunks) |
| Vector index | derived | ✅ (rebuilt from embeddings) |
| Citation extractions | derived | ✅ (re-run through eyecite) |
| `routing_rules` | SQLite only | ❌ — these are config. **Action:** export to `<workspace>/.mike/routing_rules.json` periodically. |
| `mcp_tokens` | SQLite only | ❌ — tokens are revoked on rebuild. **Action:** re-issue after rebuild. |
| Sync cursors (`sync_cursors`) | SQLite only | ❌ — next sync becomes a full sync. **Action:** acceptable cost. |
| Audit log | append-only file | ✅ (separate file) |
| Secrets (`secrets.enc`) | encrypted file | ✅ (separate file) |
| Job queue (`jobs`) | SQLite only | ❌ — in-flight jobs lost. **Action:** acceptable; idempotent jobs re-enqueue on next trigger. |

**Action items in v1.1:** export-on-write of routing rules and MCP token hashes to small JSON files in `<workspace>/.mike/state/`. Then rebuild can restore them.

## Repair (incremental rebuild)

Most failures are partial, not catastrophic. The repair flow:

1. Startup spot-check finds N items with hash mismatch (or unparseable frontmatter, or missing file referenced in SQLite).
2. Enqueue `reindex_item` job for each.
3. Job: re-read `.md`, re-validate, re-insert/update item row, re-chunk, re-embed.
4. Items appear in search again once their job completes.
5. Audit log: `repair.item_reindexed` per item.

Repair runs in the background, doesn't block startup, doesn't take the workspace lock. Reported as a banner: "Repairing 14 items; search may be incomplete."

## Disaster recovery procedures

### Scenario: `mike.db` is gone

1. Start Mike. Unlock with PIN.
2. Backend detects missing DB, prompts "Rebuild from disk?"
3. Full rebuild runs.
4. Mike is usable when foreground completes; search is fully accurate when background completes.

**Estimated time:** 5 min foreground + 30 min background for a 10k-item workspace on a modern Mac.

### Scenario: `<workspace>/matters/` is gone but `mike.db` survives

Bad. The matter files are the source of truth; SQLite is the index. Without the matters folder there's nothing to index.

**Recovery:** restore from backup. The backend refuses to start with an empty `matters/` if the DB has matter records (suggests user pointed at wrong folder).

### Scenario: Some matter folders are gone

E.g., user moved a matter folder to archive and forgot. On startup:
- Spot-check fails for that matter's items.
- Background repair marks those items as `deleted_at` (soft delete, 30-day grace period).
- After 30 days, hard-delete from SQLite.

If the user puts the folder back within 30 days, startup picks it up again and repair re-indexes.

### Scenario: External edits introduced invalid frontmatter

E.g., user opened `.md` in TextEdit and broke the YAML.
- Watcher fires.
- Parser fails.
- Item is marked `parse_error` (visible in UI with the error message).
- Search excludes it.
- User opens in a real editor, fixes it; next watcher event re-indexes.

### Scenario: Mike crashes mid-write

- `.md.tmp` file exists; `.md` is the previous version.
- Startup repair removes `.tmp` files older than 60s.
- Item is intact at its prior state.
- SQLite is unchanged.
- User's last save attempt is lost, but no corruption.

## Rebuild correctness tests

End-to-end test suite (`tests/e2e/rebuild.rs`):

1. **Round-trip:** create workspace with 100 items via API, `cargo run rebuild`, verify search returns same results.
2. **Subset rebuild:** delete random 10 items, rebuild, verify they reappear.
3. **Corrupted DB:** zero out `mike.db`, restart, rebuild, verify state.
4. **External edits:** modify 10 `.md` files between rebuilds, verify changes reflected.
5. **Missing attachment:** delete one attachment binary, rebuild, verify referencing item has a `missing_attachment` warning.
6. **Schema drift:** start with old schema, point new binary at it, verify migrations + rebuild work.
