# PLAN.md

> Master build plan. Read this first. Cross-references live in `docs/`.

## Vision

A sovereign, single-user legal practice platform. Rust backend (trunk) for performance and concurrency. Electron shell for window/lock/workspace-picker. Python sidecars (Docling, eyecite, future) for ecosystem access. Next.js frontend and Microsoft Word add-in as surfaces. MCP server exposes everything to Claude Desktop. **All cite-able content lives on disk as Markdown with YAML frontmatter** under a user-picked workspace folder; SQLite (encrypted with SQLCipher) plus sqlite-vec is a derivable index, not the source of truth.

The user owns their data in human-readable form. The app is a viewer and editor for that data.

## Repo layout

```
mike/
├── Cargo.toml                     ← Rust workspace root
├── package.json                   ← npm workspaces root
├── pyproject.toml                 ← uv workspace root
├── Justfile                       ← unified task runner
├── electron-builder.yml           ← packaging config
├── PLAN.md
├── README.md
├── docs/
│   ├── 00-anti-patterns.md
│   ├── 01-workspace-layout.md
│   ├── 02-item-frontmatter.md
│   ├── 03-sidecars.md
│   ├── 04-versioning.md
│   ├── 05-edges.md
│   ├── 06-mikeprj.md
│   ├── 07-rebuilds.md
│   ├── 08-security-model.md
│   └── 09-performance.md
├── electron/                      ← TS, Electron shell
├── backend/                       ← Rust, the trunk (forked from MikeRust)
├── sidecars/                      ← Python (Docling, eyecite, …)
├── frontend/                      ← Next.js
├── word-addin/                    ← Office add-in (ported from ebubekirkupe)
├── packages/shared/               ← polyglot type hub (auto-generated)
├── scripts/                       ← codegen + packaging
└── tests/                         ← cross-language E2E
```

Detail in `docs/05-edges.md` for what crosses between each top-level area.

## The phases

Phases are sequential. Each gates the next via acceptance criteria. Do not start phase N+1 until N's criteria pass.

### Phase 0 — Specs and skeleton (~10 days)

**Goal:** Every contract written before any code. Anti-patterns codified.

**Deliverables:**
- All ten docs in `docs/` filled in (this document set).
- Repo skeleton matching the tree above. Empty `Cargo.toml` / `package.json` / `pyproject.toml` at appropriate levels.
- Cargo workspace + npm workspaces + uv workspaces all resolve.
- `Justfile` with stub commands: `just dev`, `just build`, `just test`, `just codegen`, `just package`. Each prints "not yet implemented, see PLAN.md".
- CI scaffold (GitHub Actions) running `just typecheck` and `just test` per push. Both no-ops initially; pipeline exists.

**Acceptance:**
- `git clone && just dev` prints a specific "not yet implemented" message.
- All ten docs answer concrete questions about the system's contracts.

### Phase 1 — Reshape the trunk (~2 weeks)

**Goal:** Electron + Rust backend + workspace + lock work end-to-end. No new features yet, just transplant.

**Steps:**
1. Fork MikeRust into a fresh repo. Strip `src-tauri/`, `corpora/eurlex.rs`, `corpora/italian_legal.rs`, `routes/eurlex.rs`, `routes/italian_legal.rs`, `migrations/0015_documents_corpus.sql`, `migrations/0016_italian_legal.sql`, `aws-sdk-s3`, `aws-config`. Keep the `LegalCorpusAdapter` trait as a template for future source adapters.
2. Set up Cargo workspace under `backend/`. `cargo build --release` produces `target/release/mike-backend`.
3. Replace MikeRust's `data/` paths: every literal becomes `<workspace>/.mike/...` via a `workspace::Paths` helper. Backend refuses to start without `WORKSPACE_PATH` env var.
4. Port Electron shell from existing work: `electron/src/main.ts`, `lock/`, `workspace.ts`, `auth.ts`, `secrets.ts`, `jwt.ts`, `logging.ts`.
5. Implement `electron/src/backend.ts`: spawn the Rust binary with `WORKSPACE_PATH`, `MIKE_BACKEND_PORT=AUTO`, `MIKE_JWT_SECRET=<PIN-derived>`. Wait for `READY` token on stdout. Supervisor logic per `docs/05-edges.md`.
6. Implement `electron/src/safe-env.ts` — env scrubbing for spawned processes per `docs/08-security-model.md`.
7. Workspace lock file at `<workspace>/.mike/runtime/workspace.lock` (`{pid, hostname, started_at, mike_version}`). Refuse to open if locked by a live process; offer stale-lock takeover otherwise.
8. Wire SQLCipher: backend builds against `rusqlite` with `bundled-sqlcipher-vendored-openssl`. Database key derived from PIN via Argon2id (same flow as `secrets.enc`).
9. Backend binds to `127.0.0.1` only. Unit test asserts this; CI fails if anything binds elsewhere.
10. Host header middleware on every route. Reject anything not `127.0.0.1` / `localhost` / Word-add-in expected hostnames. Returns 421.
11. Auth middleware on every route, including `/health`. No exemptions.
12. Tracing setup: every request gets `X-Request-Id`; propagated to sidecar calls; structured logs with span IDs.
13. `Justfile`: `just dev`, `just build`, `just test`, `just codegen`, `just package` — all functional.

**Acceptance:**
- Fresh checkout, `just dev`. Lock screen appears. Set PIN. Pick a fresh folder. Backend boots. Frontend loads. Upload a PDF, chat with it, see the answer with citations. Close. Reopen. State persists.
- Rust backend never reads or writes outside `<workspace>/.mike/` plus user-picked attachments.
- `file <workspace>/.mike/mike.db` reports "data," not "SQLite 3.x database."
- Two Mike instances on the same workspace cannot both run.
- Killing Electron kills the backend within 5 seconds.
- Killing the backend externally is detected and surfaced within 3 seconds.
- `curl http://127.0.0.1:<port>/health` returns 401 without a token.
- `curl -H "Host: evil.com" ...` returns 421.

### Phase 2 — Type-sharing pipeline (~1 week)

**Goal:** Rust is the source of truth for every wire type. Changing a struct in Rust causes a TS compile error in the frontend if a callsite is now wrong.

**Steps:**
1. Add `ts-rs`, `schemars`, `utoipa` derives to every wire-format Rust type in `backend/src/types/`.
2. `cargo test` regenerates `packages/shared/src/types/*.ts` via `ts-rs`.
3. New binary `backend/bin/emit-openapi.rs` writes `packages/shared/openapi.json` from `utoipa` route annotations.
4. `scripts/codegen-client.sh` runs `openapi-typescript` against `openapi.json`, emits `packages/shared/src/client.gen.ts`.
5. Hand-written wrapper at `packages/shared/src/client.ts`: `MikeClient` class with pluggable `getAuthToken`, retry, error mapping.
6. `scripts/codegen-pydantic.sh` runs `datamodel-code-generator` against `openapi.json`, emits `sidecars/_shared/mike_shared/models.py`.
7. CI verification (`scripts/verify-codegen.sh`): re-run codegen, `git diff --exit-code`.
8. Migrate frontend imports from inline types to `@mike/shared`.

**Acceptance:**
- Add a new field to a Rust type, run `just codegen`, the frontend's `tsc` flags every callsite missing the field.
- CI fails on a PR that changes a Rust type without committing the regenerated TS / Python.

### Phase 3 — Sidecar pattern (~1 week)

**Goal:** A reusable contract for spawning, supervising, and calling Python sidecars. Docling is the proof.

**Steps:**
1. `backend/src/sidecars/trait.rs` — `Sidecar` trait + `SidecarConcurrency` enum + `SidecarHandle`. Spec in `docs/03-sidecars.md`.
2. `backend/src/sidecars/supervisor.rs` — spawn, wait-for-ready, version-check, restart-with-backoff, kill.
3. `sidecars/docling/` — FastAPI app, runtime-file writer, `GET /health` / `GET /version` / `POST /parse`. `pyproject.toml` pinned, `uv sync`-able.
4. `backend/src/sidecars/docling.rs` — `Sidecar` impl + typed `DoclingClient`.
5. Wire Docling into document-upload: prefer Docling for PDFs with `page_count > 5` or detected tables; pdfium fallback for simple text-only PDFs.
6. Multi-worker config: Docling declares `MultiWorker { default: 2, max: 4 }`. Supervisor runs `uvicorn --workers N`.
7. Sidecar version mismatch surfaces as `degraded: sidecar=docling, reason=version-mismatch` on a `/system/status` endpoint; routes needing the sidecar return 503 with `X-Sidecar-Required` header.
8. Documentation finalized in `docs/03-sidecars.md`.

**Acceptance:**
- Backend boots, Docling sidecar spawns, runtime file appears < 5s, `/health` returns 200.
- `kill -9 <docling_pid>` triggers supervisor restart within 5s; logs show request_id continuity for any in-flight request.
- Parse a 50-page litigation PDF with tables; markdown output preserves table structure.
- Parse two 50-page docs concurrently; wall time < 1.6× single-doc time (proves worker parallelism).
- Sidecar's Pydantic `ParseResponse` round-trips with Rust's `ParsedDocument` losslessly.
- Mismatched sidecar version puts backend in `degraded` state with a clear banner.

### Phase 4 — Markdown storage layer (~2 weeks)

**Goal:** Every cite-able item is a `.md` file with YAML frontmatter. SQLite is rebuildable from disk. This is the architectural keystone — do not skip ahead.

**Steps:**
1. `backend/src/storage/frontmatter.rs` — parse/serialize via `serde_yaml`. `Frontmatter` enum tagged by `kind`.
2. `backend/src/storage/layout.rs` — path schema per `docs/01-workspace-layout.md`. Slug derivation, collision handling.
3. `backend/src/storage/item.rs` — atomic writes: `.md.tmp` → fsync → rename. Two-phase write coordination (`.md` first, SQLite second, repair on startup).
4. `backend/src/storage/attachment.rs` — hash-keyed (sha256) binary store with ref-counting. Attachments live inside the matter folder, not a global pool.
5. Migration `0017_storage_layer.sql` — new `items`, `chunks`, `attachments` tables; `chunks_fts` (FTS5) and `chunks_vec` (sqlite-vec, partition-keyed by `(user_id, matter_id)`).
6. `backend/src/db/rebuild.rs` — walk `<workspace>/matters/**/items/*.md` via the `ignore` crate, parse frontmatter, populate SQLite, re-chunk, queue embed jobs.
7. Startup repair: spot-check 10 random items, content_hash mismatch → enqueue re-index.
8. Filesystem watcher (`notify` crate) with write-fence: in-memory set of "paths I just wrote in the last 2 seconds" suppresses self-triggered events.
9. Per-kind body normalizers: `document` (Docling MD), `email` (HTML → MD via `html2md` crate), `note`, `appointment`, `contact`, `chat`.
10. Frontmatter schema versioning per `docs/04-versioning.md`.

**Acceptance:**
- Create an item via API. Verify `.md` exists at expected path with correct frontmatter and body. Verify SQLite row matches.
- Manually edit a sentence in a `.md` body via VS Code; filesystem watcher fires; search reflects the new text within 5 seconds.
- `rm <workspace>/.mike/mike.db`; restart backend; index rebuild completes; search returns same results.
- `cp -r <workspace> <copy>`; open `<copy>` in a second Mike instance (on a different machine); state identical.
- `tar czf matter.tar.gz <workspace>/matters/<client>/<matter>/` produces a self-contained matter archive.
- No spurious "external edit detected" events during internal writes.

### Phase 5 — Surface elegance (~2 weeks)

**Goal:** SSE event bus end-to-end. Word add-in functioning. Cross-surface live updates real.

**Steps:**
1. `backend/src/events/bus.rs` — tokio `broadcast` channel + `MikeBridgeEvent` enum (from `@mike/shared`).
2. `backend/src/routes/events.rs` — axum SSE endpoint. Initial ping, 20s heartbeat, per-client subscription, graceful close on disconnect.
3. Publish in every mutation route: `events.publish(event)` after a successful write.
4. `frontend/src/hooks/useMikeEvents.ts` — fetch-based SSE reader (not `EventSource`, so we can send bearer tokens). Exponential backoff reconnect.
5. React Query integration: per-event-type cache invalidation in a central event router.
6. Port `word-addin/` from ebubekirkupe; update `manifest.xml` for our HTTPS port; update API client to use `@mike/shared`.
7. `backend/src/certs/` — generate self-signed cert at first launch via `rcgen` crate. Store at `<workspace>/.mike/certs/`. Prefer `~/.office-addin-dev-certs/` if present.
8. Static-serve add-in bundle at `/addin/*` on HTTPS:3002 with `Cache-Control: no-store`.
9. Pair-codes flow: `pair_codes` table, `/pair/start` and `/pair/exchange` routes, Settings UI. 60-second TTL. Exchanges for a long-lived bearer.
10. Audit log entries for: auth events, MCP token issue/revoke, cross-matter access, bulk reads, `.mikeprj` export. Append to `<workspace>/.mike/logs/audit.log`.

**Acceptance:**
- Two frontend tabs open. Create a matter in one. Other tab updates without refresh.
- Word add-in installed in Microsoft Word. Sideload manifest. Pair via Settings. Upload doc in frontend; add-in sidebar reflects the new doc within 200ms.
- Heartbeats keep the SSE connection alive across a 5-minute idle period.
- Kill backend; both surfaces enter "reconnecting" state; on backend restart, both reconnect within 30s.
- Audit log contains entries for the day's operations.

### Phase 6 — MCP server (~1 week)

**Goal:** Claude Desktop connects to Mike, lists matters, searches, reads items.

**Steps:**
1. Add `rmcp` crate. Two transports: stdio (for desktop AI clients) and Streamable HTTP on `:3003` (for remote agents).
2. `mcp_tokens` table: `(id, label, token_hash, scope, created_at, last_used_at)`. Tokens stored as Argon2id hashes; plaintext returned once at creation.
3. Read-only tools v1: `list_matters`, `get_matter(id)`, `search(query, matter_id?, kind?, limit?)`, `read_item(id)`, `find_in_item(item_id, query)`, `list_recent_items(matter_id?, since?, limit?)`.
4. Tool schemas auto-derive from Rust types via `schemars`.
5. Isolation enforcement: any tool that reads from a `strict` matter never joins across matters, even when the query looks generic. Tested.
6. Settings UI: list active tokens, "Issue new token" with label + scope, "Revoke" action.
7. `docs/MCP_CLIENT_SETUP.md` — exact lines for Claude Desktop's `claude_desktop_config.json`.

**Acceptance:**
- Issue a read-only token from Settings. Configure Claude Desktop. Claude lists "mike" as a connected server.
- "What matters do I have for Acme Corp?" returns correct list via `list_matters`.
- "Summarize the most recent email in the Stevens matter" calls `list_recent_items` then `read_item` and returns a summary.
- Strict-isolation matter: cross-matter search excludes its items automatically.

### Phase 7 — Eyecite, matter graph, unified item layer (~3–4 weeks)

**Goal:** Practice management primitives. Citations parsed automatically. Matters have clients and isolation modes. `.mikeprj` export/import.

**Steps:**
1. `sidecars/eyecite/` — FastAPI app exposing `POST /extract` and `POST /resolve`. Same shape as Docling.
2. Wire into chunker: batch chunk text through eyecite; store citations in `chunk_citations` join table.
3. Matter graph schema (migration `0018_matter_graph.sql`): `clients`, `matters` tables. `matter.isolation_mode` in `{ shared, strict }`.
4. `<workspace>/matters/<client-slug>/client.md` and `.../<matter-slug>/matter.md` files mirror the SQLite rows.
5. `chunks_vec` re-partitioned by `(user_id, matter_id)` if not already. Tests prove strict matters cannot leak.
6. Manual item routing UI: select unfiled, "Assign to matter" → move `.md`, update frontmatter, update SQLite, publish SSE.
7. Auto-routing rules v1: `routing_rules` table with from-address / domain / subject-regex / calendar-organizer matches. Evaluated at adapter ingestion.
8. `.mikeprj` export/import per `docs/06-mikeprj.md`. AES-256-GCM, Argon2id, weak email pinning.
9. MCP write tools: `create_note`, `append_to_item`, `assign_item`. Scoped to `read_write` tokens only.

**Acceptance:**
- Create client + matter via API; verify on-disk paths exist with correct frontmatter.
- Set matter to `isolation_mode: strict`. Cross-matter search excludes it. Direct SQL query trying to bypass partition keys errors (sqlite-vec rejects).
- Index a 50-page brief; query "show chunks citing Bell Atlantic v. Twombly"; returns chunks via `chunk_citations` join.
- Export `.mikeprj` for `colleague@firm.example`; peer Mike instance with that email opens it; refuses for any other email.

### Phase 8 — Sources: job queue + Gmail + IMAP + CalDAV (~3+ weeks per source)

**Goal:** Email and calendar flow into matters automatically.

**Steps:**
1. SQLite-backed job queue: `jobs` table (`id`, `kind`, `payload_json`, `state`, `attempts`, `max_attempts`, `next_run_at`, `started_at`, `completed_at`, `last_error`, `idempotency_key`). Single tokio worker loop in the backend.
2. OAuth flow infrastructure: backend `POST /oauth/<provider>/start` returns auth URL; Electron opens it externally; backend listens on `127.0.0.1:<random>/callback`; tokens encrypted into `secrets.enc`.
3. Gmail adapter: client wrapping Gmail API, sync state in `sync_cursors`, polling job re-enqueued every 5 minutes, push via Gmail watch where available.
4. Per-message: fetch headers + body + attachments, normalize to email Item `.md`, run routing rules, enqueue chunk + embed jobs.
5. IMAP adapter using `async-imap` + `mailparse` crates; same normalize path.
6. CalDAV adapter using `reqwest` + the relevant RFCs; recurring-event handling deferred to v1.1.
7. Google Calendar adapter via Google APIs.
8. `SourceAdapter` trait formalized: `full_sync`, `incremental_sync`, `schedule()`.
9. Routing rules expanded: from-address, domain, subject regex, calendar-organizer matching. "Pending review" badge for borderline items.
10. SSE events for sync: `source.sync.started`, `source.sync.progress`, `source.sync.completed`.
11. MCP tools: `list_recent_emails`, `list_appointments`.

**Acceptance:**
- Connect Gmail. Initial sync of 30 days runs in background; frontend shows live progress.
- Define routing rule: `*@stevens-counsel.com` → Stevens matter. Test email lands in matter folder on next poll.
- Open matter folder in Finder; emails are real `.md` files with full body and attachments.
- Word add-in shows updated email count immediately via SSE.
- Claude Desktop: "what's the latest from Stevens counsel?" returns the email via MCP.

## Cross-cutting concerns

**Testing.** Rust unit tests per module. Rust integration tests against axum's test client with temp workspaces. Sidecar contract tests with shared fixtures. Playwright E2E from Phase 5 onward.

**Observability.** Every Rust module uses `tracing`. Logs rotate at `<workspace>/.mike/logs/`. Request IDs propagate cross-process. No remote telemetry, no anonymous metrics, no exceptions.

**Packaging.** `just package`:
1. `cargo build --release` → `target/release/mike-backend`
2. PyInstaller bundles each sidecar (Python interpreter + deps + ONNX models) into a single binary
3. `next build` (Next.js standalone) for frontend
4. `webpack` for Word add-in
5. `electron-builder` consumes all of the above as `extraResources`

macOS code signing + notarization is a separate script using `electron-builder`'s notarize hook. Requires Apple Developer account.

**CI matrix.** macOS-latest (Apple Silicon + Intel) and Linux for backend tests. Windows added when Windows Hello path is implemented.

**Versioning.** Repo version in root `package.json`. Sidecar API versions independent (each sidecar has `__version__`). Workspace `schema_version` per `docs/04-versioning.md`. `.md` frontmatter `schema_version` per-item.

## Alpha definition

Alpha-ready when **all** of the following pass:

1. Lock → workspace → backend → frontend path rock-solid.
2. Upload PDF → Docling → `.md` on disk → searchable → cited.
3. SSE works across two frontend tabs and the Word add-in.
4. Claude Desktop connects via MCP and queries matters.
5. Gmail source: connect, sync, route, see in matters.
6. Workspace portable via `cp -r` to another machine.
7. Delete `mike.db`, restart, rebuild from disk succeeds within 5 min for 10k items.
8. Export/import `.mikeprj` round-trip works.
9. `mike.db` opaque on disk (SQLCipher proven).
10. Two Mike instances on same workspace cannot both run.
11. External `.md` edits reflect within 5 seconds without spurious self-triggers.
12. Killing a sidecar externally triggers automatic restart within 5s with request_id continuity.
13. All HTTP endpoints (including `/health`) reject requests without auth.
14. Host header bound to anything except `127.0.0.1`/`localhost`/add-in expected returns 421.
15. Sidecar version mismatch → `degraded` state + UI banner; no silent fallback.

Estimated total: **14–18 weeks of focused work.** Faster if Rust is comfortable; longer if learning concurrently.

## Out of scope for v1

- Outlook add-in
- Mobile companion
- Multi-user / firm-wide install
- Time tracking / billing
- E-filing integrations
- Macro-OCR for scanned PDFs (add when first user needs)
- Vision-LLM for image-only documents
- Cross-matter conflict-of-interest detection (matter-graph analysis)
- Web/SaaS deployment
- Touch ID on macOS (defer to v1.1 per `docs/08-security-model.md`)
- Windows Hello on Windows (defer to Windows port)
- v2 `.mikeprj` with authenticated sharing (defer)

Anti-patterns and process rules in `docs/00-anti-patterns.md`. Read before contributing.
