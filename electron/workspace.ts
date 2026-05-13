import { app, dialog } from "electron";
import * as fs from "fs";
import * as path from "path";

interface AppConfig {
  lastWorkspace?: string;
}

const CONFIG_FILE = "config.json";
const MIKE_DIR = ".mike";

function configPath(): string {
  return path.join(app.getPath("userData"), CONFIG_FILE);
}

export function readConfig(): AppConfig {
  try {
    const raw = fs.readFileSync(configPath(), "utf8");
    return JSON.parse(raw) as AppConfig;
  } catch {
    return {};
  }
}

export function writeConfig(cfg: AppConfig): void {
  atomicWriteFileSync(configPath(), JSON.stringify(cfg, null, 2));
}

export function isWorkspaceValid(workspace: string | undefined): boolean {
  if (!workspace) return false;
  try {
    const stat = fs.statSync(workspace);
    if (!stat.isDirectory()) return false;
    fs.accessSync(workspace, fs.constants.R_OK | fs.constants.W_OK);
    return true;
  } catch {
    return false;
  }
}

export function ensureMikeDir(workspace: string): string {
  const mikeDir = path.join(workspace, MIKE_DIR);
  fs.mkdirSync(mikeDir, { recursive: true });
  fs.mkdirSync(path.join(mikeDir, "runtime"), { recursive: true });
  fs.mkdirSync(path.join(workspace, "matters", "_unfiled", "items"), { recursive: true });
  fs.mkdirSync(path.join(workspace, "matters", "_unfiled", "attachments"), { recursive: true });
  return mikeDir;
}

function isInsideInstallTree(workspace: string): boolean {
  const candidates = [app.getAppPath()];
  // process.resourcesPath is only set under Electron; guard for type safety.
  const resourcesPath = (
    process as NodeJS.Process & { resourcesPath?: string }
  ).resourcesPath;
  if (resourcesPath) candidates.push(resourcesPath);
  const ws = path.resolve(workspace);
  for (const c of candidates) {
    const installRoot = path.resolve(c);
    const rel = path.relative(installRoot, ws);
    if (rel === "" || (!rel.startsWith("..") && !path.isAbsolute(rel))) {
      return true;
    }
  }
  return false;
}

export async function pickWorkspace(): Promise<string | null> {
  const result = await dialog.showOpenDialog({
    title: "Choose a Mike workspace folder",
    properties: ["openDirectory", "createDirectory"],
    message:
      "Mike will store all of your documents, settings, and database in this folder.",
  });
  if (result.canceled || result.filePaths.length === 0) return null;
  const rawPicked = result.filePaths[0];
  // realpath defeats junctions/symlinks that point into the install tree.
  let picked: string;
  try {
    picked = fs.realpathSync(rawPicked);
  } catch {
    picked = path.resolve(rawPicked);
  }
  if (!isWorkspaceValid(picked)) {
    await dialog.showMessageBox({
      type: "error",
      message: "Selected folder is not readable/writable. Please pick another.",
    });
    return null;
  }
  if (isInsideInstallTree(picked)) {
    await dialog.showMessageBox({
      type: "error",
      message:
        "Workspace cannot live inside the Mike install directory. " +
        "Please pick a folder elsewhere (e.g. inside Documents).",
    });
    return null;
  }
  ensureMikeDir(picked);
  return picked;
}

export function authFilePath(workspace: string): string {
  return path.join(workspace, MIKE_DIR, "pin.json");
}

export function secretsFilePath(workspace: string): string {
  return path.join(workspace, MIKE_DIR, "secrets.enc");
}

export function authStateFilePath(workspace: string): string {
  return path.join(workspace, MIKE_DIR, "auth-state.json");
}

export function runtimeFilePath(workspace: string): string {
  return path.join(workspace, MIKE_DIR, "runtime.json");
}

export function backendRuntimeFilePath(workspace: string): string {
  return path.join(workspace, MIKE_DIR, "runtime", "backend.json");
}

export function workspaceLockFilePath(workspace: string): string {
  return path.join(workspace, MIKE_DIR, "runtime", "workspace.lock");
}

function pidIsAlive(pid: number): boolean {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return (err as NodeJS.ErrnoException).code === "EPERM";
  }
}

function readLockPid(file: string): number | null {
  try {
    const parsed = JSON.parse(fs.readFileSync(file, "utf8")) as { pid?: unknown };
    return typeof parsed.pid === "number" ? parsed.pid : null;
  } catch {
    return null;
  }
}

export function acquireWorkspaceLock(workspace: string): void {
  ensureMikeDir(workspace);
  const file = workspaceLockFilePath(workspace);
  const payload = JSON.stringify(
    {
      version: 1,
      pid: process.pid,
      startedAt: new Date().toISOString(),
    },
    null,
    2,
  );

  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      fs.writeFileSync(file, payload, { flag: "wx", mode: 0o600 });
      return;
    } catch (err) {
      const code = (err as NodeJS.ErrnoException).code;
      if (code !== "EEXIST") throw err;
      const pid = readLockPid(file);
      if (pid && pid !== process.pid && pidIsAlive(pid)) {
        throw new Error(
          `This workspace is already open in another Mike process (pid ${pid}).`,
        );
      }
      try {
        fs.unlinkSync(file);
      } catch {
        // Retry once; if another process wins the race, the next write fails.
      }
    }
  }
  fs.writeFileSync(file, payload, { flag: "wx", mode: 0o600 });
}

export function releaseWorkspaceLock(workspace: string): void {
  const file = workspaceLockFilePath(workspace);
  const pid = readLockPid(file);
  if (pid !== null && pid !== process.pid) return;
  try {
    fs.unlinkSync(file);
  } catch {
    // Already gone.
  }
}

/**
 * Atomic write — writes to a temp file then renames over the destination.
 * Avoids leaving a half-written file if power loss / crash interrupts.
 */
export function atomicWriteFileSync(
  dest: string,
  data: string | Buffer,
  opts: { mode?: number } = {},
): void {
  const tmp = `${dest}.${process.pid}.${Date.now()}.tmp`;
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.writeFileSync(tmp, data, { mode: opts.mode });
  fs.renameSync(tmp, dest);
}
