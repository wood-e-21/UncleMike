# docs/00-anti-patterns.md

> Read this before writing code. Each item below has burned someone in the past. We decide them once, here, and don't relitigate them in PRs.

## The ten rules

### 1. No business logic in Electron main
Electron is a shell. It owns: lock screen, PIN entry, workspace folder picker, window lifecycle, dock icon, deep links (`mike://...`), menu bar, child process supervision. Everything else — auth, storage, search, sync, AI calls — lives in the Rust backend.

**Why:** Electron's main process is JavaScript with full Node access. Putting state there means we have two languages owning state, two paths for bugs to hide. A clean Electron lets us swap it for Tauri or a CLI later without rewriting Mike.

### 2. No types defined outside `packages/shared`
Every wire-format type comes from `@mike/shared`. If you find yourself writing the same interface in two languages, the codegen is broken — fix the codegen.

**Why:** Drift between frontend and backend was the #1 source of bugs in the ebubekirkupe lineage. Single source of truth makes it impossible.

### 3. No HTTP endpoint without auth + Host validation
Every route requires a valid bearer token. Every router has Host-header middleware that rejects anything other than `127.0.0.1`, `localhost`, or the Word-add-in expected hostnames. **No exemptions, not even `/health`.**

**Why:** A rogue same-user process that knows the port shouldn't be able to do anything useful. DNS rebinding is a real attack (`docs/08-security-model.md`).

### 4. No SQLite access from anywhere except `backend/src/db/`
Frontend never queries the DB. Sidecars never query the DB. Electron never queries the DB. All access goes through the Rust backend's HTTP API.

**Why:** One process owns the database. That process owns transactions, migrations, encryption, and consistency. Multiple writers means corruption, race conditions, and impossible debugging.

### 5. No `.md` writes from anywhere except `backend/src/storage/`
The Rust backend is the only writer to `<workspace>/matters/`. Frontends never write directly. External edits via VS Code/Obsidian are read-only-by-Mike except via the filesystem watcher path which re-indexes them.

**Why:** Single writer means atomic writes and the filesystem-watcher write-fence work. Two writers means race conditions on every save.

### 6. No direct frontend → sidecar calls
Always frontend → backend → sidecar. The backend owns sidecar lifecycle, version compatibility, supervision. Bypassing it means sidecars have to be exposed on routable ports with their own auth, which doubles the attack surface.

**Why:** Sidecars are internal. They aren't authenticated independently. The backend is the membrane.

### 7. No silent fallbacks
If a sidecar is down, fail explicitly with a clear error and a `X-Sidecar-Required` header. Don't quietly route to a worse path.

**Why:** Silent fallbacks hide outages until they compound. A loud failure is fixable; a silent degradation makes everything mysteriously slow and we lose hours hunting it.

### 8. No background work without a `jobs` row
After Phase 7, every async operation that needs to survive a restart is in the job queue. Ad-hoc `tokio::spawn` for ingestion / sync / embedding is forbidden.

**Why:** Without persistence, a crash mid-sync loses work and the user doesn't know. The queue makes recovery automatic and the state inspectable.

### 9. No env-var-passed secrets after backend startup
At spawn time, Electron passes `MIKE_JWT_SECRET` (and only that — see `safe-env.ts`) via env. After startup, secrets come from `secrets.enc` (AES-256-GCM, key from PIN via Argon2id). The backend never re-emits secrets to env for any child process. Sidecars do not need secrets.

**Why:** Env vars are inheritable, dumpable, sometimes logged. Once the backend has unlocked, secrets live in memory only.

### 10. No `0.0.0.0` binding
Every listener binds explicitly to `127.0.0.1`. There is no scenario in v1 where `0.0.0.0` is correct. A unit test in `backend/src/lib.rs` asserts this; CI fails if anything binds elsewhere.

**Why:** Defense in depth. If the user is on coffee-shop wifi, no part of Mike should be reachable from the network. One mistake here is catastrophic; the test makes it impossible.

## Process rules

### PR template
Every PR answers:
- Which phase / step does this implement?
- Which acceptance criterion does it move toward?
- Does it touch any of rules 1–10 above? If yes, why is that acceptable?
- Did you regenerate types? Did you commit them?

### Decision log
When we make a judgment call that future-us will second-guess, log it as a one-paragraph entry in `docs/decisions.md` (created on first use). Include: the decision, the alternatives considered, the date.

### No new languages
Rust, TypeScript, Python. Not Go, not Swift, not Kotlin, not anything else, no matter how nice the library. If the only way to do X is language Y, write a sidecar wrapping the existing X-in-Y library and call it via HTTP from Rust.

### No new transports
HTTP for everything except Edge 1 / Edge 2 spawn handshakes (env vars + stdout). No WebSockets, no gRPC, no shared memory, no named pipes. SSE is HTTP. JSON-RPC over stdio is the one exception, only because MCP requires it.
