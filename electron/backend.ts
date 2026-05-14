import { ChildProcess, spawn } from "child_process";
import * as fs from "fs";
import * as path from "path";
import { appResources } from "./paths";
import { backendRuntimeFilePath } from "./workspace";
import { setLogRedactions } from "./logging";
import { safeChildEnv } from "./safeEnv";

interface SpawnOptions {
  workspace: string;
  backendUnlockSecret: string;
}

interface ExitInfo {
  code: number | null;
  signal: NodeJS.Signals | null;
}

let backendProc: ChildProcess | null = null;
let backendPort: number | null = null;
let backendWorkspace: string | null = null;
let backendToken: string | null = null;
let lastExitInfo: ExitInfo | null = null;

export function isBackendRunning(): boolean {
  return backendProc !== null && !backendProc.killed && backendProc.exitCode === null;
}

function backendBinary(): { cmd: string; args: string[]; cwd: string; envExtra?: NodeJS.ProcessEnv } {
  const root = path.resolve(__dirname, "..");
  if (process.env.NODE_ENV === "development") {
    return {
      cmd: "cargo",
      args: ["run", "-p", "mike-backend", "--bin", "mike-backend"],
      cwd: root,
    };
  }
  const resources = appResources();
  const name = process.platform === "win32" ? "mike-backend.exe" : "mike-backend";
  return {
    cmd: path.join(resources, "backend", name),
    args: [],
    cwd: resources,
  };
}

function safeEnv(opts: SpawnOptions): NodeJS.ProcessEnv {
  const env = safeChildEnv();
  env.WORKSPACE_PATH = opts.workspace;
  env.MIKE_BACKEND_PORT = "AUTO";
  env.MIKE_BACKEND_UNLOCK_SECRET = opts.backendUnlockSecret;
  return env;
}

export function spawnBackend(opts: SpawnOptions): void {
  if (isBackendRunning()) return;
  lastExitInfo = null;
  backendPort = null;
  backendWorkspace = opts.workspace;
  setLogRedactions([opts.backendUnlockSecret]);

  try {
    fs.unlinkSync(backendRuntimeFilePath(opts.workspace));
  } catch {
    // stale runtime not present
  }

  const bin = backendBinary();
  console.log(`[backend] spawning: ${bin.cmd} ${bin.args.join(" ")} (cwd=${bin.cwd})`);
  backendProc = spawn(bin.cmd, bin.args, {
    cwd: bin.cwd,
    env: safeEnv(opts),
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  backendProc.on("error", (err) => {
    console.error("[backend] spawn error:", err);
  });
  backendProc.stdout?.on("data", (b: Buffer) => {
    const s = b.toString();
    process.stdout.write(`[backend] ${s}`);
    if (s.includes("READY")) {
      const rt = readRuntimeFile();
      if (rt?.port) backendPort = rt.port;
    }
  });
  backendProc.stderr?.on("data", (b: Buffer) => {
    const s = b.toString();
    process.stderr.write(`[backend] ${s}`);
  });
  backendProc.on("exit", (code, signal) => {
    console.log(`[backend] exited code=${code} signal=${signal}`);
    lastExitInfo = { code, signal };
    backendProc = null;
  });
}

export function setBackendToken(token: string | null): void {
  backendToken = token;
}

export function stopBackend(): void {
  if (backendProc && !backendProc.killed) {
    backendProc.kill();
  }
  backendProc = null;
  backendPort = null;
  backendWorkspace = null;
  backendToken = null;
  setLogRedactions([]);
}

function readRuntimeFile(): { port?: number } | null {
  if (!backendWorkspace) return null;
  try {
    const raw = fs.readFileSync(backendRuntimeFilePath(backendWorkspace), "utf8");
    return JSON.parse(raw) as { port?: number };
  } catch {
    return null;
  }
}

export async function waitForBackend(timeoutMs = 30_000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (lastExitInfo) return false;
    if (backendPort === null) {
      const rt = readRuntimeFile();
      if (rt?.port) backendPort = rt.port;
    }
    if (backendPort !== null && backendToken) {
      try {
        const resp = await fetch(`http://127.0.0.1:${backendPort}/health`, {
          headers: { Authorization: `Bearer ${backendToken}` },
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

export function getBackendPort(): number {
  if (backendPort !== null) return backendPort;
  const rt = readRuntimeFile();
  if (rt?.port) {
    backendPort = rt.port;
    return rt.port;
  }
  return 3001;
}

export function getBackendApiBase(): string {
  return `http://127.0.0.1:${getBackendPort()}`;
}

export function getBackendExitInfo(): ExitInfo | null {
  return lastExitInfo;
}
