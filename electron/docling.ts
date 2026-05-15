/**
 * Docling sidecar lifecycle — Phase 1 spawn shim.
 *
 * What this module does TODAY:
 *   - Locates a Python ≥3.11 with `docling` importable
 *   - Spawns `python <sidecar_app>`
 *   - Waits for the sidecar to write its runtime file at
 *     `<workspace>/.mike/runtime/sidecars/docling.json`
 *   - Health-checks /health on the OS-assigned port
 *   - On Electron shutdown, sends SIGTERM (then the OS reaps SIGKILL)
 *
 * What this module is NOT:
 *   - It is NOT the Rust supervisor. The Rust side
 *     (`backend/src/sidecars/supervisor.rs`) reads the same runtime
 *     file and is the one routes consult to decide whether a request
 *     can be served. This module's job is just the spawn-and-watch
 *     plumbing that Phase 3 will replace with `tokio::process::Command`.
 *
 * Behavior on "Docling unavailable":
 *   - If MIKE_DOCLING_ENABLED=0 (explicit opt-out), we don't spawn;
 *     the supervisor reports `down`; the backend returns 503 with
 *     `X-Sidecar-Required: docling@1` to any route that needs it.
 *     There is NO silent fallback to legacy extractors — per
 *     anti-pattern #7. The frontend surfaces the banner.
 *   - If Python isn't found, same thing: we don't spawn, supervisor
 *     reports down, requests requiring Docling return 503.
 *
 * Default-on: Docling is enabled by default. To opt out for
 * development, set MIKE_DOCLING_ENABLED=0 explicitly.
 */

import { ChildProcess, spawn, spawnSync } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { appResources } from "./paths";
import { safeSidecarEnv } from "./safeEnv";

interface SpawnOptions {
  workspace: string;
}

interface ExitInfo {
  code: number | null;
  signal: NodeJS.Signals | null;
}

let doclingProc: ChildProcess | null = null;
let doclingPort: number | null = null;
let doclingWorkspace: string | null = null;
let lastExitInfo: ExitInfo | null = null;

/**
 * Phase C4: default-on. Set MIKE_DOCLING_ENABLED=0 to opt out
 * explicitly (handy for dev iteration when you don't want a 30s
 * Docling-load delay on every restart). Any other value — unset,
 * empty, "1" — enables the sidecar.
 */
export function isDoclingEnabled(): boolean {
  return process.env.MIKE_DOCLING_ENABLED !== "0";
}

export function isDoclingRunning(): boolean {
  return (
    doclingProc !== null && !doclingProc.killed && doclingProc.exitCode === null
  );
}

/**
 * Per docs/01-workspace-layout.md, sidecar runtime files live under
 * `<workspace>/.mike/runtime/sidecars/<name>.json`. The legacy
 * `<workspace>/.mike/docling-runtime.json` path is no longer used.
 * If a Phase-1 workspace still has the old file, the supervisor
 * silently ignores it (it looks at the new path).
 */
export function doclingRuntimePath(workspace: string): string {
  return path.join(workspace, ".mike", "runtime", "sidecars", "docling.json");
}

export function doclingModelCacheDir(workspace: string): string {
  // Stay inside .mike/sidecar-cache/<name> per docs/01-workspace-layout.md.
  return path.join(workspace, ".mike", "sidecar-cache", "docling");
}

/**
 * Locate a Python interpreter capable of running the sidecar. The sidecar
 * needs Python ≥3.11 and `docling` + `fastapi` + `uvicorn` installed.
 *
 * Probe order:
 *   1. MIKE_DOCLING_PYTHON env override
 *   2. python3.12, python3.11 from PATH
 *   3. python3 from PATH (only if --version reports >= 3.11)
 */
export function findPython(): string | null {
  const override = process.env.MIKE_DOCLING_PYTHON;
  if (override && fileIsExecutable(override)) {
    return override;
  }

  const candidates = ["python3.12", "python3.11", "python3"];
  const pathDirs = (process.env.PATH ?? "").split(path.delimiter).filter(Boolean);
  for (const name of candidates) {
    for (const dir of pathDirs) {
      const candidate = path.join(dir, name);
      if (!fileIsExecutable(candidate)) continue;
      const version = pythonVersion(candidate);
      if (version && version[0] === 3 && version[1] >= 11) {
        return candidate;
      }
    }
  }
  return null;
}

function fileIsExecutable(p: string): boolean {
  try {
    fs.accessSync(p, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function pythonVersion(executable: string): [number, number, number] | null {
  try {
    const r = spawnSync(executable, ["--version"], {
      stdio: ["ignore", "pipe", "pipe"],
      timeout: 3000,
    });
    if (r.status !== 0) return null;
    const out = (r.stdout?.toString() ?? "") + (r.stderr?.toString() ?? "");
    const m = out.match(/Python\s+(\d+)\.(\d+)\.(\d+)/);
    if (!m) return null;
    return [Number(m[1]), Number(m[2]), Number(m[3])];
  } catch {
    return null;
  }
}

function sidecarAppPath(): string {
  return path.join(appResources(), "python", "docling_sidecar", "app.py");
}

// safeSpawnEnv kept as an alias for the shared helper so the spawn
// site below reads naturally. Allowlist lives in electron/safeEnv.ts.
const safeSpawnEnv = safeSidecarEnv;

export function spawnDocling(opts: SpawnOptions): boolean {
  if (!isDoclingEnabled()) {
    console.log("[docling] MIKE_DOCLING_ENABLED!=1, skipping sidecar spawn");
    return false;
  }
  if (isDoclingRunning()) return true;

  const python = findPython();
  if (!python) {
    console.warn(
      "[docling] no Python ≥3.11 found on PATH (set MIKE_DOCLING_PYTHON to override). " +
        "Skipping sidecar; backend will use legacy extractors.",
    );
    return false;
  }

  const appPy = sidecarAppPath();
  if (!fs.existsSync(appPy)) {
    console.warn(`[docling] sidecar entry not found at ${appPy}; skipping`);
    return false;
  }

  // Clear any stale runtime file so waitForDocling doesn't read the
  // previous run's port.
  try {
    fs.unlinkSync(doclingRuntimePath(opts.workspace));
  } catch {
    // not present — fine
  }
  fs.mkdirSync(path.dirname(doclingRuntimePath(opts.workspace)), {
    recursive: true,
  });
  fs.mkdirSync(doclingModelCacheDir(opts.workspace), { recursive: true });

  lastExitInfo = null;
  doclingPort = null;
  doclingWorkspace = opts.workspace;

  // Universal envelope (docs/03-sidecars.md §"Spawn-time env vars")
  // plus Docling-specific runtime tuning. We pass BOTH the universal
  // names and the legacy MIKE_DOCLING_* names for the Phase 1 →
  // Phase 3 transition. The Phase 3 Rust supervisor will set only
  // MIKE_SIDECAR_*.
  const env: NodeJS.ProcessEnv = {
    ...safeSpawnEnv(),
    MIKE_SIDECAR_NAME: "docling",
    MIKE_SIDECAR_RUNTIME: doclingRuntimePath(opts.workspace),
    MIKE_SIDECAR_CACHE_DIR: doclingModelCacheDir(opts.workspace),
    MIKE_SIDECAR_PARENT_PID: String(process.pid),
    // Legacy aliases — drop in Phase 3.
    MIKE_DOCLING_RUNTIME: doclingRuntimePath(opts.workspace),
    MIKE_DOCLING_CACHE_DIR: doclingModelCacheDir(opts.workspace),
    MIKE_DOCLING_PARENT_PID: String(process.pid),
    // Apple Silicon default; user can override to "cpu" or "cuda".
    MIKE_DOCLING_DEVICE:
      process.env.MIKE_DOCLING_DEVICE ??
      (os.platform() === "darwin" ? "mps" : "cpu"),
    // Pass through chunker config if user set it.
    ...(process.env.MIKE_DOCLING_MAX_TOKENS
      ? { MIKE_DOCLING_MAX_TOKENS: process.env.MIKE_DOCLING_MAX_TOKENS }
      : {}),
  };

  console.log(`[docling] spawning: ${python} ${appPy}`);
  doclingProc = spawn(python, [appPy], {
    cwd: path.dirname(appPy),
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  doclingProc.on("error", (err) => {
    console.error("[docling] spawn error:", err);
  });
  doclingProc.stdout?.on("data", (b: Buffer) => {
    const s = b.toString();
    process.stdout.write(`[docling] ${s}`);
  });
  doclingProc.stderr?.on("data", (b: Buffer) => {
    const s = b.toString();
    process.stderr.write(`[docling] ${s}`);
  });
  doclingProc.on("exit", (code, signal) => {
    console.log(`[docling] exited code=${code} signal=${signal}`);
    lastExitInfo = { code, signal };
    doclingProc = null;
  });
  return true;
}

export function stopDocling(): void {
  if (doclingProc && !doclingProc.killed) {
    doclingProc.kill();
  }
  doclingProc = null;
  doclingPort = null;
  if (doclingWorkspace) {
    try {
      fs.unlinkSync(doclingRuntimePath(doclingWorkspace));
    } catch {
      // not present — fine
    }
  }
  doclingWorkspace = null;
}

function readRuntimeFile(): { port?: number } | null {
  if (!doclingWorkspace) return null;
  try {
    const raw = fs.readFileSync(doclingRuntimePath(doclingWorkspace), "utf8");
    return JSON.parse(raw) as { port?: number };
  } catch {
    return null;
  }
}

/**
 * Wait until the sidecar has written its runtime file AND responds on
 * /health. Returns false on timeout or if the sidecar has already exited.
 *
 * Default timeout is 60s — generous because the first run may also be
 * loading the layout + TableFormer models. /health responds immediately,
 * but the parent can call this to confirm a healthy state before opening
 * the main window.
 */
export async function waitForDocling(timeoutMs = 60_000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (lastExitInfo) return false;
    if (doclingPort === null) {
      const rt = readRuntimeFile();
      if (rt?.port) doclingPort = rt.port;
    }
    if (doclingPort !== null) {
      try {
        const resp = await fetch(`http://127.0.0.1:${doclingPort}/health`, {
          signal: AbortSignal.timeout(1000),
        });
        if (resp.ok) return true;
      } catch {
        // not ready yet
      }
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  return false;
}

export function getDoclingPort(): number | null {
  if (doclingPort !== null) return doclingPort;
  const rt = readRuntimeFile();
  if (rt?.port) {
    doclingPort = rt.port;
    return rt.port;
  }
  return null;
}

export function getDoclingExitInfo(): ExitInfo | null {
  return lastExitInfo;
}
