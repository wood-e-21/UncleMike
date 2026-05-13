/**
 * Docling sidecar lifecycle.
 *
 * Mirrors electron/backend.ts: spawn a Python FastAPI/Uvicorn server next
 * to the Express backend, wait for it to write its runtime file, then
 * healthcheck. The Express backend reads
 *    <workspace>/.mike/docling-runtime.json
 * to discover the sidecar URL — no IPC plumbing required.
 *
 * Phase 1 packaging: requires the user's Python (≥3.11) to have docling
 * installed (see python/docling_sidecar/requirements.txt). If Python or
 * the docling import is missing, the sidecar fails to come up and the
 * Express backend silently falls back to the legacy pdfjs-dist / mammoth
 * extractors. Phase 2 will replace this with a PyInstaller-bundled binary
 * declared in package.json > build.extraResources.
 *
 * Gated by env: only spawned when MIKE_DOCLING_ENABLED=1.
 */

import { ChildProcess, spawn, spawnSync } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { appResources } from "./paths";

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

export function isDoclingEnabled(): boolean {
  return process.env.MIKE_DOCLING_ENABLED === "1";
}

export function isDoclingRunning(): boolean {
  return (
    doclingProc !== null && !doclingProc.killed && doclingProc.exitCode === null
  );
}

export function doclingRuntimePath(workspace: string): string {
  return path.join(workspace, ".mike", "docling-runtime.json");
}

export function doclingModelCacheDir(workspace: string): string {
  return path.join(workspace, ".mike", "docling-cache");
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

function safeSpawnEnv(): NodeJS.ProcessEnv {
  // Mirror backend/src/lib/safeSpawn.ts: never leak the parent's secrets to
  // the child. Pass only the basics needed to find Python's site-packages
  // and write to a temp dir.
  const env: NodeJS.ProcessEnv = {};
  for (const k of [
    "PATH",
    "TEMP",
    "TMP",
    "TMPDIR",
    "HOME",
    "LANG",
    "LC_ALL",
    "PYTHONPATH",
    "VIRTUAL_ENV",
  ]) {
    const v = process.env[k];
    if (v !== undefined) env[k] = v;
  }
  return env;
}

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

  const env: NodeJS.ProcessEnv = {
    ...safeSpawnEnv(),
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
