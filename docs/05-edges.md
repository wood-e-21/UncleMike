# docs/05-edges.md

> The five process edges. What crosses each, with what transport and what auth.

## Process topology

Five distinct processes (plus N sidecars):

1. **Electron main** (Node.js, TS) — shell, window, lock, workspace, child supervision
2. **Rust backend** (axum) — API, storage, search, MCP, sidecar supervision
3. **Python sidecars** (FastAPI) — Docling, eyecite, …
4. **Next.js frontend** (browser-side, runs inside Electron's BrowserWindow)
5. **Word add-in** (browser-side, runs inside Word's task pane)

Plus external clients:
- **Claude Desktop** (or other MCP client) — runs entirely outside Mike

## The edges

### Edge 1: Electron ⇄ Rust backend

**Direction:** Electron spawns; backend serves.
**Transports:**
- Spawn-time: env vars + child stdout handshake (one-shot, at start).
- Steady-state: HTTP on `http://127.0.0.1:<port>` (`<port>` from backend's runtime file).

**Spawn handshake:**
```
Electron                                   Rust backend
   │                                              │
   │ Command::new("mike-backend")                 │
   │   .env("WORKSPACE_PATH", "/Users/...")       │
   │   .env("MIKE_BACKEND_PORT", "AUTO")          │
   │   .env("MIKE_JWT_SECRET", "<32-bytes>")      │
   │   .spawn()                                   │
   ├─────────────────────────────────────────────▶│
   │                                              │ read env vars
   │                                              │ open SQLCipher with PIN-derived key
   │                                              │ bind 127.0.0.1:<random>
   │                                              │ write runtime/backend.json
   │                                              │ print "READY\n"
   │              "READY\n"                       │
   │◀─────────────────────────────────────────────┤
   │ read runtime/backend.json for port           │
   │ ready to make HTTP calls                     │
   │                                              │
```

**Auth (steady-state):** Bearer JWT signed with `MIKE_JWT_SECRET`. JWT includes `user: "local"`, `exp: <24h>`, `iat: <now>`. Issued by Electron, passed to frontend at startup via preload IPC, included on every API call.

**Shutdown:** SIGTERM, wait 5s, SIGKILL.

**What crosses:**
- Electron → backend: every HTTP API call the frontend makes.
- Backend → Electron: nothing. Backend never calls Electron.

**What does NOT cross:**
- Electron does not read or write SQLite, `.md`, or any user data.
- Backend does not invoke Electron APIs (no native menu items, no notifications). If we want notifications, backend publishes an SSE event and the frontend forwards to Electron via preload IPC.

### Edge 2: Rust backend ⇄ Python sidecars

**Direction:** Backend spawns; sidecar serves.
**Transports:**
- Spawn-time: env vars + runtime file + stdout `READY`.
- Steady-state: HTTP on `http://127.0.0.1:<sidecar_port>`.

Full contract in `docs/03-sidecars.md`.

**Auth:** None at the wire level — sidecars only listen on 127.0.0.1 and are not exposed beyond the backend. Sidecars never receive secrets.

**What crosses:**
- Backend → sidecar: parse requests, extraction requests, citation requests. Plain text + binary input as JSON (base64 for binaries).
- Sidecar → backend: JSON responses with structured output.

**What does NOT cross:**
- Sidecars never call the backend's API.
- Sidecars never touch SQLite or filesystem outside their cache dir.

### Edge 3: Rust backend ⇄ frontend (and Word add-in)

**Direction:** Browser/webview connects; backend serves.
**Transports:**
- HTTP on `:3001` for the Next.js frontend (loopback-only).
- HTTPS on `:3002` for the Word add-in (Office requires HTTPS).
- SSE on both ports at `/events`.

**Auth:**
- Frontend: bearer JWT (see Edge 1).
- Word add-in: long-lived bearer token, obtained via pair-codes flow.

**Pair-codes flow:**
```
Settings UI                Frontend           Backend           Word add-in
     │                        │                  │                    │
     │ user clicks "Pair      │                  │                    │
     │  Word add-in"          │                  │                    │
     ├───────────────────────▶│                  │                    │
     │                        │ POST /pair/start │                    │
     │                        ├─────────────────▶│                    │
     │                        │                  │ generate code      │
     │                        │                  │ (6 digits, TTL 60s)│
     │                        │  {code: "428751"}│                    │
     │                        │◀─────────────────┤                    │
     │  show "428751"         │                  │                    │
     │◀───────────────────────┤                  │                    │
     │                        │                  │                    │
     │ user types "428751" in add-in              │                    │
     │                        │                  │  POST /pair/exchange (code: 428751)
     │                        │                  │◀───────────────────┤
     │                        │                  │ verify, generate    │
     │                        │                  │ bearer token, store │
     │                        │                  │  {token: "mt_..."}  │
     │                        │                  ├───────────────────▶│
     │                        │                  │                    │ store in Office storage
     │                        │                  │                    │ use on every request
```

**SSE event format:**
```
data: {"type": "document.created", "document": {...}}\n
\n
```

20s heartbeat (`{"type": "ping", "ts": ...}`) keeps the connection alive.

**What crosses:**
- Frontend → backend: API calls (CRUD on matters, items, chats), search queries, chat completions (with SSE streaming).
- Word add-in → backend: same set, scoped to its bearer token's allowed routes.
- Backend → both surfaces: SSE event stream for live updates.

### Edge 4: Rust backend ⇄ MCP clients

**Direction:** External AI client connects; backend serves.
**Transports:** Two, both supported:
- **stdio** — backend spawned as a child of Claude Desktop, JSON-RPC over stdin/stdout.
- **Streamable HTTP** on `:3003`.

**Wire format:** JSON-RPC 2.0 per the MCP spec.

**Auth:** MCP token (long bearer, prefix `mt_`). Token issued from Mike Settings → MCP. Stored hashed (Argon2id) in `mcp_tokens` table.

For stdio: token passed as env var `MIKE_MCP_TOKEN` when Claude Desktop spawns the MCP binary.
For HTTP: `Authorization: Bearer mt_...` header.

**Scopes:**
- `read`: list_matters, get_matter, search, read_item, find_in_item, list_recent_items, list_recent_emails, list_appointments
- `read_write`: above + create_note, append_to_item, assign_item

**Isolation enforcement:** any tool that touches data from a `strict`-isolation matter never joins across matters. Tested.

**What crosses:**
- Claude Desktop → backend: tool calls.
- Backend → Claude Desktop: tool responses (read results, search hits, item bodies).

### Edge 5: Type sharing (build-time)

Not a runtime edge — a development-time pipeline. Rust types are the source of truth; everything else is regenerated.

```
   backend/src/types/*.rs                 (Rust — source of truth)
       │
       ├─ #[derive(TS, JsonSchema, Serialize, Deserialize)]
       │
       ▼
   cargo test            cargo run --bin emit-openapi
       │                         │
       ▼                         ▼
   packages/shared/src/types/*.ts    packages/shared/openapi.json
                                        │
       ┌────────────────────────────────┴────────────────────┐
       │                                                     │
       ▼                                                     ▼
   openapi-typescript                          datamodel-code-generator
       │                                                     │
       ▼                                                     ▼
   packages/shared/src/client.gen.ts        sidecars/_shared/mike_shared/models.py
       │                                                     │
       ▼                                                     ▼
   frontend, word-addin import {…}             sidecars from mike_shared.models import …
```

All generated files are **committed to git**. Consumers don't need a Rust toolchain. CI re-runs codegen and fails on diff.

## Summary table

| Edge | From | To | Transport | Auth | Direction of spawn |
|---|---|---|---|---|---|
| 1 | Electron | Rust backend | env vars + HTTP | JWT | Electron → backend |
| 2 | Rust backend | Python sidecar | env vars + HTTP | none (loopback only) | backend → sidecar |
| 3a | Frontend | Rust backend | HTTP + SSE | JWT | (no spawn) |
| 3b | Word add-in | Rust backend | HTTPS + SSE | bearer token (pair codes) | (no spawn) |
| 4 | MCP client | Rust backend | stdio or HTTP, JSON-RPC | MCP token (env or header) | client → backend (stdio) |
| 5 | Rust source | TS + Python | codegen | — | build-time only |
