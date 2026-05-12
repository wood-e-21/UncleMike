# docs/03-sidecars.md

> The contract every Python sidecar implements. Lifecycle, transport, versioning, supervision.

## What a sidecar is

A standalone Python process providing one or more capabilities the Rust backend doesn't natively have. Examples: Docling (PDF/DOCX extraction with layout), eyecite (legal citation parsing). Future: presidio (PII redaction), blackstone (legal NER), tesseract (OCR).

Sidecars **never** call the backend, **never** touch the database, **never** read or write `<workspace>/matters/`. They are pure compute services: receive bytes / text in, return JSON out.

## Lifecycle

```
              ┌────────────────────────────────────────────┐
              │              Rust backend                  │
              │                                            │
   spawn ────▶│ supervisor:                                │
              │   1. resolve Python interpreter             │
              │   2. set env vars                          │
              │   3. tokio::process::Command::spawn         │
              │   4. wait for runtime file (30s timeout)   │
              │   5. read {port, pid, version, caps}        │
              │   6. version-check; refuse if mismatch     │
              │   7. GET /health × 2; refuse if fail        │
              │   8. expose typed client to rest of app    │
              │                                            │
              │   on child exit:                           │
              │     log, backoff (1→2→4→…→30s), restart    │
              │                                            │
              │   on shutdown:                             │
              │     SIGTERM, wait 5s, SIGKILL              │
              └────────────────────────────────────────────┘
                              ▲
                  HTTP        │           env vars
                  /health     │           via Command::env
                  /version    │           ────────────┐
                  /<work>     │                       │
                              │                       ▼
                          ┌───────────────────────────────┐
                          │       Python sidecar           │
                          │                                │
                          │  startup:                      │
                          │   1. read MIKE_SIDECAR_RUNTIME │
                          │   2. bind 127.0.0.1:0          │
                          │   3. write runtime file        │
                          │   4. print "READY" to stdout   │
                          │   5. uvicorn serve forever     │
                          └───────────────────────────────┘
```

## Spawn-time env vars

Every sidecar receives exactly these env vars and **only** these. No inheritance from the backend's process env (see `safe-env.rs` for the allowlist):

| Variable | Required | Meaning |
|---|---|---|
| `MIKE_SIDECAR_NAME` | yes | `"docling"`, `"eyecite"`, etc. |
| `MIKE_SIDECAR_RUNTIME` | yes | absolute path for runtime JSON file |
| `MIKE_SIDECAR_CACHE_DIR` | yes | absolute path; sidecar may create, read, write |
| `MIKE_SIDECAR_LOG` | yes | absolute path for log file |
| `PATH` | yes | inherited from system |
| `HOME` | yes | inherited |
| `TMPDIR` / `TEMP` / `TMP` | yes | inherited |
| `LANG` / `LC_ALL` | optional | inherited if set |

The sidecar's working directory is set to its bundled installation directory. It must not depend on cwd for any file resolution.

## Runtime file format

The sidecar writes this **after** binding to its port but **before** printing `READY`:

```json
{
  "port": 53718,
  "pid": 8421,
  "version": "1.0.0",
  "schema_version": 1,
  "capabilities": ["parse", "rechunk"],
  "started_at": "2026-05-11T09:30:45Z"
}
```

| Field | Meaning |
|---|---|
| `port` | TCP port the sidecar is listening on (always on 127.0.0.1) |
| `pid` | process ID — used by supervisor for liveness check |
| `version` | semver. **Major version** must match what the backend expects. |
| `schema_version` | independent of code version; describes request/response shape |
| `capabilities` | string array of supported operations |
| `started_at` | for log correlation |

The supervisor reads this file, then writes its own `<workspace>/.mike/runtime/sidecars/<name>.json` mirror with extra fields (`process_handle`, `restart_count`).

## Required endpoints

Every sidecar exposes at minimum:

### `GET /health`
Returns 200 with `{"ok": true}` if the sidecar can serve requests. No auth; this is on `127.0.0.1` only. Used by supervisor for liveness checks (every 30s).

### `GET /version`
Returns `{"version": "1.0.0", "schema_version": 1, "capabilities": [...]}`. Used by supervisor on startup and after restart to verify nothing changed.

### Sidecar-specific work endpoints
Documented per-sidecar. All accept and return JSON. All run on 127.0.0.1 only.

## Sidecar trait (Rust side)

```rust
#[async_trait]
pub trait Sidecar: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn expected_major_version(&self) -> u32;
    fn entry_module(&self) -> &'static str;          // e.g. "docling_sidecar"
    fn concurrency(&self) -> SidecarConcurrency;
    fn extra_env(&self) -> HashMap<String, String> { Default::default() }
}

pub enum SidecarConcurrency {
    SingleWorker,
    MultiWorker { default: u32, max: u32 },
}
```

`MultiWorker` is implemented by running uvicorn with `--workers N`, which forks N OS processes. Each worker loads its own copy of the model. Memory cost is `N * model_size`; documented per-sidecar.

| Sidecar | Concurrency | Notes |
|---|---|---|
| docling | `MultiWorker { default: 2, max: 4 }` | ~1–2GB per worker (Docling models) |
| eyecite | `MultiWorker { default: 1, max: 2 }` | ~200MB per worker, light |
| (future) presidio | `MultiWorker { default: 1, max: 2 }` | ~400MB per worker |
| (future) ocr-tesseract | `MultiWorker { default: 1, max: 4 }` | minimal RAM, CPU-heavy |

## Version compatibility

Backend declares `expected_major_version()` per sidecar. On startup:
- Sidecar version major == expected: proceed.
- Mismatch: supervisor kills the sidecar, logs error, sets system status to `degraded: sidecar=<name>, reason=version-mismatch`.

Routes that need a degraded sidecar return:
```
HTTP/1.1 503 Service Unavailable
Retry-After: 60
X-Sidecar-Required: docling@1
Content-Type: application/json

{"error": "Sidecar 'docling' is unavailable (version-mismatch). Please update Mike."}
```

The frontend reads `X-Sidecar-Required`, renders a banner. **No silent fallback to a different parser path** per anti-pattern #7.

## Failure modes and recovery

| Failure | Detection | Response |
|---|---|---|
| Sidecar fails to spawn (binary missing) | `Command::spawn` errors | Log, set `degraded`, retry every 60s |
| Sidecar starts but never writes runtime file | 30s timeout in supervisor | SIGKILL, backoff, retry |
| Sidecar writes runtime file but `/health` fails | health probe times out | SIGKILL, backoff, retry |
| Sidecar exits cleanly (code 0) | `child.wait()` resolves | Log, restart immediately |
| Sidecar exits with error (code != 0) | `child.wait()` resolves | Log stderr tail, backoff, restart |
| Sidecar hangs | `/health` returns >5s | Log, SIGKILL, restart |
| Out-of-memory | OS kills child, `wait` returns signal | Log, backoff longer (10s min), restart |

Restart backoff: 1s, 2s, 4s, 8s, 16s, 30s, 30s, … (capped). Reset to 1s after 5 minutes of stable runtime.

## Logging

Sidecars log to `<workspace>/.mike/logs/sidecar-<name>.log`. Format: structured JSON, one event per line. Required fields: `timestamp`, `level`, `request_id` (when servicing a request), `message`.

Sidecars receive their log path via `MIKE_SIDECAR_LOG` and configure their logger accordingly. The supervisor also captures stderr to the same file (prefixed).

Sidecars **never** write to stdout except for the single `READY` token at startup.

## Request ID propagation

The backend includes `X-Request-Id: <ulid>` on every call to a sidecar. The sidecar logs it on every event during that request. A single `grep <request-id> <workspace>/.mike/logs/*.log` shows the full causal trace across processes.

## Packaging

In dev: sidecars run from `sidecars/<name>/` via `uv run python -m <name>_sidecar`.

In production: each sidecar is bundled by PyInstaller into a single binary (`mike-sidecar-<name>`). The binary contains the Python interpreter, all dependencies, and the bundled model weights (when small enough). Large model weights are downloaded into `<workspace>/.mike/sidecar-cache/<name>/` on first use, with a progress endpoint the frontend can poll.

The `electron-builder` config copies the bundled binaries into `Mike.app/Contents/Resources/sidecars/`.

## Adding a new sidecar

1. Write the FastAPI app in `sidecars/<name>/`.
2. Add `pyproject.toml` with pinned dependencies, `uv sync`-able.
3. Define request/response types in Rust (`backend/src/types/<name>.rs`) with `ts-rs` + `schemars` + `utoipa` derives.
4. `just codegen` regenerates Pydantic models in `sidecars/_shared/`.
5. Implement `Sidecar` trait in `backend/src/sidecars/<name>.rs`.
6. Implement typed client in `backend/src/sidecars/<name>.rs` using `reqwest`.
7. Register in `lib.rs` startup with the supervisor.
8. Add acceptance tests: spawn, health, version, parse, version-mismatch, kill-and-restart.
9. Update `electron-builder.yml` to bundle the new sidecar.

Estimated effort for a new sidecar: 2–3 days once the pattern is known.
