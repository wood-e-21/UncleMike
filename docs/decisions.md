# docs/decisions.md

> Architectural judgment calls that future-us would otherwise relitigate.
> One paragraph each. Dated. New entries go at the bottom.
>
> Per [`docs/00-anti-patterns.md`](00-anti-patterns.md#decision-log): when
> we make a call that wasn't already in the canonical docs, log it here.

---

## 2026-05-12 — Labeled-HMAC key derivation (HKDF-style)

**Decision.** From the user's PIN we run Argon2id once to a 64-byte
root, then HMAC-SHA256(root, label) to mint as many purpose-specific
keys as we need. Current label inventory:

| Label | Holder | Purpose |
|---|---|---|
| `pin-verifier` | `electron/auth.ts` → `pin.json` | confirm a PIN attempt |
| `backend-unlock` | env var → backend | further-derived keys |
| `jwt-verification` | both sides | sign/verify JWTs |
| `secrets-bundle` | both sides | AES-256-GCM key for `secrets.enc` |
| `sqlcipher` | backend only | SQLCipher database key |

**Alternative considered.** Re-run Argon2id per purpose (one
"sqlcipher-pin", one "secrets-pin", etc.). Rejected: Argon2id is
intentionally expensive (~100ms with our params); fanning out via
cheap HMAC labels is the standard HKDF pattern and gives the same
key separation.

**Why this matters.** Anyone reading the SQLCipher pragma in
`backend/src/db/cipher.rs` should be able to find the matching
HMAC label in `electron/keys.ts`. They MUST agree string-for-string;
otherwise the database opens with the wrong key and looks corrupt.

---

## 2026-05-12 — Secrets keystore: AES-256-GCM with PIN-derived key

**Decision.** API keys (Anthropic, Gemini, OpenRouter, OpenAI,
Resend) live in `<workspace>/.mike/secrets.enc`. The file format
is:

```
bytes 0..2     "MS" magic
byte 2         format version (currently 0x01)
bytes 3..15    12-byte AES-GCM nonce
bytes 15..end  AES-256-GCM ciphertext + 16-byte auth tag
```

The encryption key is `HMAC-SHA256(unlock_secret, "secrets-bundle")`,
where `unlock_secret` is the PIN-derived root described above.

The bundle is loaded into the backend's `AppState.secrets` (in-memory
only) at session start via `POST /internal/secrets/load`. LLM modules
read from `state.secrets`; environment variables for API keys are
NOT consulted (anti-pattern #9).

**Alternatives considered.**
- *Electron `safeStorage`* (the Phase-0/1 reshape's first attempt).
  Rejected because: (a) it ties decryption to the OS keychain, which
  defeats backup portability — copy `secrets.enc` to another machine,
  it can't decrypt; (b) the threat model in Decision 2 of
  `docs/08-security-model.md` was specifically about leaked backups,
  and OS-keychain-bound files are worst-case there.
- *`secrets.enc` keyed by SQLCipher's database key*. Rejected because
  it would couple the bundle's portability to the database's, and
  one of the two might want to be restorable independently
  (e.g. clearing the DB while keeping API keys).

**Cost.** Same Electron-side decryption work as before; the only
durable difference is that the secrets file now travels with the
workspace.

**Migration note.** `user_settings.claude_api_key` /
`gemini_api_key` columns are still read as a fallback during the UI
transition. Plan: drop those columns when Account → Models writes
through `POST /internal/secrets/save` instead of `PUT /user/settings`.

---

## 2026-05-12 — SQLCipher via `bundled-sqlcipher-vendored-openssl`

**Decision.** The backend's database file (`<workspace>/.mike/mike.db`)
is encrypted with SQLCipher. `libsqlite3-sys` is configured at the
workspace root with the `bundled-sqlcipher-vendored-openssl` feature;
cargo's feature unification propagates this to sqlx. The cipher key
is derived per the labeled-HMAC pattern above.

**On every connection** the pool sets:

```sql
PRAGMA key = "x'<hex>'";
PRAGMA cipher_compatibility = 4;
PRAGMA cipher_memory_security = ON;
```

The hex form (`x'…'`) bypasses SQLCipher's PBKDF2 over a quoted
string, so the raw bytes are used as-is. This means our cipher key
never goes through SQLCipher's KDF, only ours (Argon2id +
labeled HMAC).

**Alternative considered.** `rusqlite` instead of sqlx — rejected
because the existing routes use sqlx heavily; switching would have
been weeks of work for no security delta.

**Cost.** Bundled OpenSSL adds ~30s to a clean build. SQLCipher
itself is ~10% query overhead, which is well below the budget for
a single-user app.

---

## 2026-05-12 — Sidecar supervisor — interface in Rust, spawn in Electron (Phase 1)

**Decision.** The `Sidecar` trait and the `Supervisor` struct live
in `backend/src/sidecars/`. Routes consult
`state.sidecars.state("docling")` and either proceed or return 503 +
`X-Sidecar-Required: docling@1` per `docs/03-sidecars.md`.
**Today (Phase 1)**, Electron still spawns the Python process; the
Rust supervisor reads the runtime file Electron wrote and probes
`/health` + `/version`. **Phase 3** moves spawning to Rust via
`tokio::process::Command`; nothing in routes or the supervisor's
public API changes.

**Why this split.** Spawning Python from Electron unblocked the
Phase-1 reshape weeks earlier than waiting for a real Rust
supervisor would have. Putting the trait and supervisor in Rust
*now* means routes can be written today as if the supervisor were
Rust-native; only the implementation behind the trait changes when
Phase 3 lands.

**Anti-pattern alignment.** Anti-pattern #6 says the backend is the
membrane. The supervisor *interface* is the membrane; routes never
talk to sidecars directly. The Electron-side spawn is plumbing,
not authority.

**Cost.** A small amount of duplication while Phase 1 lasts:
`electron/docling.ts` knows the runtime-file path and env var
names that `backend/src/sidecars/supervisor.rs` also knows. The
docs (03-sidecars.md, 01-workspace-layout.md) are the single source
of truth for both; if they diverge, the supervisor's probe will
report `degraded`.

---

## 2026-05-12 — `_unfiled` matter is a sibling of clients on disk

**Decision.** The `_unfiled` matter's on-disk path is
`<workspace>/matters/_unfiled/matter.md`, NOT
`<workspace>/matters/<unfiled-client-slug>/_unfiled/matter.md`. The
SQLite `matters` row still has a `client_id` (pointing to a hidden
"Unfiled" client row), but `repositories::matters::write_matter_md`
short-circuits to `paths.unfiled_matter_dir()` whenever
`slug == "_unfiled"`.

**Why this matters.** This is what `docs/01-workspace-layout.md`
specifies. The Phase 1 reshape's first cut nested it under a
synthetic client. Fixed in Phase B3 / D2.

---

## 2026-05-13 — X-Request-Id propagation

**Decision.** Every HTTP request gets an `X-Request-Id`. If the
caller supplies one, we keep it; otherwise we mint a UUIDv4. The
ID is reflected in the response. The Phase 3 sidecar client will
forward it to `/parse` calls so a single grep across
`backend.log`, `sidecar-docling.log`, etc. shows the full causal
trace.

**Why UUIDv4 not ULID.** This crate doesn't already depend on
`ulid` and the ID is opaque to consumers; UUIDv4 is fine. Switch
to ULID when the audit log wants sortable IDs.

**Cost.** One `tower-http` feature (`request-id`).

---

## 2026-05-13 — `dotenvy` skipped under Electron supervision

**Decision.** When `MIKE_BACKEND_UNLOCK_SECRET` is set, the backend
treats Electron as authoritative for env and skips the `.env` walk.
Anti-pattern #9 forbids env-var-passed secrets after startup; reading
a stray `.env` from somewhere on disk would silently undermine that.
A misfiled `.env` inside `Mike.app/Contents/Resources/` is the
specific footgun this guards.

**Standalone runs** (`cargo run -p mike-backend` for tests / dev
without Electron) still walk for `.env` because there's no Electron
to inject anything otherwise.
