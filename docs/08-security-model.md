# docs/08-security-model.md

> What we defend against. What we don't. The decisions logged once so future-us doesn't relitigate them.

## Threat actors we model

In order of likelihood and impact:

1. **Stolen laptop / lost USB drive.** Single biggest realistic threat. Device with the workspace folder ends up in someone else's hands.
2. **Leaked backup.** Time Machine on an unencrypted external drive, Backblaze with a weak password, a misplaced .zip of the user's home directory.
3. **Malware running as the same user.** A keylogger / RAT installed via a malicious email or app. Has filesystem access, can read process memory, can attach debuggers (on permissive OS configs).
4. **Web-based DNS rebinding.** User visits a malicious site; the site uses DNS tricks to make their browser contact Mike's local API.
5. **Curious bystander.** Someone briefly at the user's machine while they're away — looking, not malware-installing.
6. **Compromised internet connection.** Wi-Fi attacker on the same network.
7. **Compromised LLM API endpoint.** Mike sends data to Claude/Gemini/OpenAI; that endpoint is intercepted.

## What we explicitly do NOT defend against

8. **Root / admin on the machine.** Root can read everything, attach to any process, dump memory, defeat any local crypto. Out of scope.
9. **State-level adversaries with physical access.** Cold-boot attacks, hardware key extraction. Out of scope.
10. **Compromised LLM provider doing post-hoc disclosure.** If Claude logs prompts, our use of Claude is a disclosure to Anthropic. Mitigation = user choice of provider, not technical.
11. **Insider threat from the user themselves.** The user can read everything. Audit logs help with accountability but not prevention.

## Decisions

For each threat, what defends it. Each decision is final until explicitly revisited.

### Decision 1: SQLCipher for `mike.db`

**Defends:** Stolen laptop (1), leaked backup (2).
**Decision:** Use SQLCipher (or `wxSQLite3` if AGPL conflicts) to encrypt the entire database file with AES-256. Key derived from PIN via Argon2id.
**Cost:** ~10% query overhead. One added dependency.
**Why:** Without this, the database file on a stolen laptop reveals everything. With it, the file is opaque bytes.

### Decision 2: AES-256-GCM secrets bundle with Argon2id KDF

**Defends:** Stolen laptop (1), leaked backup (2), malware reading files without process memory access (3).
**Decision:** AI provider API keys, OAuth refresh tokens, MCP token plaintexts (briefly, during issuance) live in `<workspace>/.mike/secrets.enc`. Decrypted into RAM only after PIN entry.
**Cost:** Adds a PIN-entry step to every Mike session. Worth it.

### Decision 3: PIN-only auth for v1; macOS Touch ID deferred to v1.1

**Defends:** Curious bystander (5).
**Decision:** PIN entered at the lock screen. Argon2id-hashed for verification. After verification, the same PIN seeds the SQLCipher and secrets keys.
**Touch ID deferred** because the implementation (`objc2-local-authentication` + `security-framework` Keychain ACLs) is roughly a week of work that doesn't move the alpha forward. Add in v1.1.
**Cost:** Slight UX friction.

### Decision 4: 127.0.0.1-only listeners

**Defends:** Compromised network (6).
**Decision:** Backend, sidecars, MCP HTTP all bind explicitly to `127.0.0.1`. Unit-tested. CI fails if any listener binds elsewhere.
**Why:** A misbind to `0.0.0.0` exposes the API to anyone on the user's network (coffee shop, conference Wi-Fi). The test makes the mistake impossible to ship.

### Decision 5: Host header validation

**Defends:** DNS rebinding (4).
**Decision:** Every router has Host-header middleware. Allowed: `127.0.0.1`, `localhost`, Word-add-in expected hostnames. Anything else → 421 Misdirected Request.
**Cost:** ~15 lines of code.

### Decision 6: Strict CORS per-port

**Defends:** Cross-origin attacks via browser (4).
**Decision:** Each port has its own CORS allowlist. Frontend port: `null` (Electron's `file://` origin sends `null`). Word add-in port: Office's official endpoints only. MCP HTTP port: no browser origins allowed.

### Decision 7: Auth required on every endpoint

**Defends:** Same-user processes (3), DNS rebinding (4).
**Decision:** No "free" routes. Every endpoint requires `Authorization: Bearer <jwt>` or `Bearer <mcp-token>`. Even `/health`. Electron passes its JWT when probing.
**Cost:** Negligible. Middleware already exists.

### Decision 8: HTTPS only where required (Word add-in), not internally

**Defends:** Nothing additional that the above don't already cover.
**Decision:** Word add-in port uses HTTPS because Office requires it. Frontend port and MCP HTTP port stay HTTP.
**Why:** Internal-loopback HTTPS is security theater. The threats it would defend (loopback sniffing) require root, which already defeats every other defense.
**Revisit if:** a compliance framework (SOC 2, HIPAA) requires TLS-everywhere and a customer demands it. Then we add it; ~1 week of work, no actual security delta.

### Decision 9: Audit log

**Defends:** Insider accountability (11), forensic recovery after any breach.
**Decision:** Append-only JSON-lines log at `<workspace>/.mike/logs/audit.log`. Logged events:
- Auth (login, logout, lockout)
- MCP token issue / revoke / use
- Cross-matter access (search hitting matter X from chat scoped to matter Y)
- Bulk reads (>50 items in one query)
- MCP write tool calls
- `.mikeprj` export/import
- Workspace open/close
**Rotation:** 50 MB max per file, 10 files kept, then oldest dropped.
**Cost:** ~50 lines of code + log rotation library.

### Decision 10: Code signing + notarization (macOS first)

**Defends:** Impostor binaries.
**Decision:** All builds signed with a Developer ID and notarized via Apple's process. Windows Authenticode signing added when Windows port lands.
**Cost:** Apple Developer Program membership + ~half-day of CI tooling per platform.

### Decision 11: Process sandboxing — deferred

**Defends:** Malware with reduced capabilities (3).
**Decision:** Deferred to post-alpha. Consider:
- macOS App Sandbox with explicit entitlements
- Linux: AppArmor/SELinux profiles when we have Linux builds
- Windows: AppContainer
**Why deferred:** Real defense in depth, but significant ongoing tax for the development cycle. Worth doing once the surface stabilizes.

### Decision 12: No telemetry, no remote logging, no analytics

**Defends:** Compromised LLM provider (7), generic supply-chain risk.
**Decision:** No outbound network calls except those the user explicitly initiates (LLM API calls, OAuth flows, mail/calendar polling, `.mikeprj` export to a file). No anonymous metrics, no crash reports, no "phone home." Logs stay on disk under the user's workspace.
**Cost:** We have no usage data. Worth it for the audience.

### Decision 13: Single-instance workspace lock

**Defends:** Data corruption from dual writers (often via cloud-sync folders).
**Decision:** `workspace.lock` file with pid + hostname. Refuse to open if locked by a live process. Document iCloud / Dropbox usage as "one machine at a time."

### Decision 14: Two-phase write with startup repair

**Defends:** Crash-mid-write corruption.
**Decision:** `.md` written atomically (tmp + rename), then SQLite updated. Startup repair handles `.md`-ahead-of-SQLite via content_hash spot-check.
**Cost:** Cheap.

### Decision 15: Workspace folder portability

**Defends:** Vendor lock-in, recovery after Mike binary is gone.
**Decision:** All user data in `<workspace>/matters/` as plain `.md` files plus binary attachments. Readable by any text editor / file browser. SQLite is fully reconstructable per `docs/07-rebuilds.md`.
**Cost:** Performance — every item write touches the filesystem. Acceptable for our throughput.

## Untaken paths and why

### Why not encrypted FUSE filesystem for `matters/`?

Considered: mount `matters/` via a userspace encrypted filesystem. Rejected:
- Adds installation complexity (FUSE on macOS requires kext-equivalent permissions).
- The plain-text-on-disk goal is intentional — lawyers must be able to read their own files without Mike running.
- SQLCipher covers the index; OS-level full-disk encryption (FileVault) covers the rest.

### Why not E2EE chat with LLM providers?

Not possible without provider cooperation. They have to read the prompt to generate the response. Mitigation = user picks a provider whose ToS they accept.

### Why not Yubikey / hardware-key unlock?

Considered for v2. Adds dependency on hardware most users don't have. PIN is enough for v1.

### Why not zero-knowledge backup encryption?

Out of scope. Users use their existing backup tools; we recommend they use ones with encryption (Time Machine, Backblaze with personal key, Arq). We don't ship a backup product.

## Compliance posture

Mike is not certified against SOC 2, HIPAA, or any other framework. The architecture has the bones to pass most of them:
- Encryption at rest (Decision 1)
- Audit logging (Decision 9)
- Auth on every endpoint (Decision 7)
- No silent telemetry (Decision 12)

But certification requires audits, policies, employee training — all out of scope until the product has paying customers who request it.

## Reporting security issues

Until we have a `security.txt`:
- Email: `security@<domain>` (set up on registration)
- Acknowledge within 48 hours
- Track in private repo, never in public issues until fixed
- Credit reporter in release notes (with permission)

## Decision log

Each decision above gets a date when first made (Phase 0). When we revisit, add a row to the decision's section with the new date and rationale.

This file is `git`-tracked; the history is the audit trail.
