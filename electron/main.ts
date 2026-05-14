import { app, BrowserWindow, dialog, ipcMain, session, shell } from "electron";
import * as fs from "fs";
import * as path from "path";
import {
  readConfig,
  writeConfig,
  isWorkspaceValid,
  pickWorkspace,
  acquireWorkspaceLock,
  releaseWorkspaceLock,
} from "./workspace";
import {
  hasPassword,
  setPassword,
  changePassword,
  unlockPassword,
  isLockedOut,
  recordFailedAttempt,
  recordSuccessfulAttempt,
} from "./auth";
import { signLocalJwt } from "./jwt";
import { deriveJwtSecretHex } from "./keys";
import {
  spawnBackend,
  stopBackend,
  waitForBackend,
  getBackendApiBase,
  getBackendExitInfo,
  setBackendToken,
} from "./backend";
import { spawnFrontend, stopFrontend, waitForFrontend } from "./frontend";
import { initLogging, getLogPath, closeLogging } from "./logging";
import { loadSecretsToBackend } from "./secrets";

const FRONTEND_URL = "http://localhost:3000";
const LOCAL_USER_ID = "local-user";
const LOCAL_USER_EMAIL = "user@local";
const JWT_TTL_SECONDS = 60 * 60 * 24; // 24h

let win: BrowserWindow | null = null;
let lockWebContents: Electron.WebContents | null = null;
let currentWorkspace: string | null = null;
let sessionJwt: string | null = null;
let sessionSecret: string | null = null;
let lockedWorkspace: string | null = null;
let unlocking = false;

function createWindow(): BrowserWindow {
  const w = new BrowserWindow({
    width: 1280,
    height: 820,
    minWidth: 800,
    minHeight: 600,
    title: "Mike",
    backgroundColor: "#0b0b0d",
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });
  w.removeMenu();

  w.webContents.on(
    "did-fail-load",
    (_event, errorCode, errorDescription, validatedURL) => {
      if (errorCode === -3) return;
      console.error(
        `[loadURL failed] code=${errorCode} desc=${errorDescription} url=${validatedURL}`,
      );
      dialog.showErrorBox(
        "Mike couldn't load",
        `Failed to open ${validatedURL}\n\n${errorDescription} (code ${errorCode})\n\n` +
          (getLogPath() ? `Check the log file:\n${getLogPath()}` : ""),
      );
    },
  );

  // Window-scoped DevTools / log shortcuts. DevTools toggles are gated on
  // dev builds and locked out while the lock screen is showing — a packaged
  // user has no business reaching DevTools, and an attacker with physical
  // access to the lock screen could otherwise read the password input.
  w.webContents.on("before-input-event", (_e, input) => {
    if (input.type !== "keyDown") return;
    const devToolsAllowed = !app.isPackaged && lockWebContents === null;
    if (input.key === "F12") {
      if (devToolsAllowed) w.webContents.toggleDevTools();
    } else if (
      (input.control || input.meta) &&
      input.shift &&
      input.key.toLowerCase() === "i"
    ) {
      if (devToolsAllowed) w.webContents.toggleDevTools();
    } else if (
      (input.control || input.meta) &&
      input.shift &&
      input.key.toLowerCase() === "l"
    ) {
      const lp = getLogPath();
      if (lp) void shell.openPath(lp);
    }
  });
  // If something else opens DevTools (renderer-initiated, etc.) while we're
  // on the lock screen, slam them shut.
  w.webContents.on("devtools-opened", () => {
    if (lockWebContents !== null || app.isPackaged) {
      w.webContents.closeDevTools();
    }
  });

  return w;
}

function loadLockScreen(w: BrowserWindow): void {
  void w.loadFile(path.join(__dirname, "lock", "lock.html"));
  lockWebContents = w.webContents;
}

function loadMainApp(w: BrowserWindow): void {
  lockWebContents = null;
  void w.loadURL(FRONTEND_URL);
}

async function startSession(workspace: string, backendUnlockSecret: string): Promise<void> {
  acquireWorkspaceLock(workspace);
  lockedWorkspace = workspace;
  sessionSecret = deriveJwtSecretHex(backendUnlockSecret);
  sessionJwt = signLocalJwt(
    sessionSecret,
    LOCAL_USER_ID,
    LOCAL_USER_EMAIL,
    JWT_TTL_SECONDS,
  );
  spawnBackend({
    workspace,
    backendUnlockSecret,
  });
  setBackendToken(sessionJwt);
}

function releaseActiveWorkspaceLock(): void {
  if (!lockedWorkspace) return;
  releaseWorkspaceLock(lockedWorkspace);
  lockedWorkspace = null;
}

function tailLogFile(maxLines = 50): string {
  const lp = getLogPath();
  if (!lp) return "(no log file)";
  try {
    const data = fs.readFileSync(lp, "utf8");
    const lines = data.trimEnd().split(/\r?\n/);
    return lines.slice(-maxLines).join("\n");
  } catch {
    return "(unable to read log)";
  }
}

ipcMain.handle("mike:getState", () => {
  const cfg = readConfig();
  const ws =
    currentWorkspace ??
    (isWorkspaceValid(cfg.lastWorkspace) ? cfg.lastWorkspace! : null);
  if (ws !== currentWorkspace) currentWorkspace = ws;
  const lock = ws
    ? isLockedOut(ws)
    : { locked: false, secondsRemaining: 0 };
  return {
    workspace: ws,
    hasPassword: ws ? hasPassword(ws) : false,
    lockedOut: lock.locked,
    lockoutSecondsRemaining: lock.secondsRemaining,
  };
});

ipcMain.handle("mike:pickWorkspace", async () => {
  // C5: don't allow workspace switch while a session is active.
  if (sessionJwt !== null) {
    return {
      ok: false,
      error: "Sign out first before switching workspaces.",
    };
  }
  const picked = await pickWorkspace();
  if (!picked) return { ok: false };
  writeConfig({ lastWorkspace: picked });
  currentWorkspace = picked;
  try {
    initLogging(picked);
  } catch (err) {
    console.warn("[pickWorkspace] failed to init log file:", err);
  }
  return { ok: true, workspace: picked, hasPassword: hasPassword(picked) };
});

// A1: only the lock screen may set the initial password. Once the renderer
// loads the main app, this IPC is closed. To rotate a password from inside
// the app, use `mike:changePassword` (requires the current password).
ipcMain.handle("mike:setPin", async (event, password: unknown) => {
  if (event.sender !== lockWebContents) {
    return {
      ok: false,
      error: "setPin can only be called from the lock screen.",
    };
  }
  if (!currentWorkspace) return { ok: false, error: "No workspace selected." };
  if (typeof password !== "string") {
    return { ok: false, error: "Invalid password input." };
  }
  if (hasPassword(currentWorkspace)) {
    return {
      ok: false,
      error:
        "A password is already set for this workspace. Use Change Password instead.",
    };
  }
  try {
    await setPassword(currentWorkspace, password);
    return { ok: true };
  } catch (err) {
    return { ok: false, error: (err as Error).message };
  }
});

ipcMain.handle(
  "mike:changePin",
  async (_event, oldPassword: unknown, newPassword: unknown) => {
    if (!currentWorkspace) {
      return { ok: false, error: "No workspace selected." };
    }
    if (typeof oldPassword !== "string" || typeof newPassword !== "string") {
      return { ok: false, error: "Invalid password input." };
    }
    if (!hasPassword(currentWorkspace)) {
      return { ok: false, error: "No password to change." };
    }
    try {
      const changed = await changePassword(currentWorkspace, oldPassword, newPassword);
      if (!changed) return { ok: false, error: "Current password is incorrect." };
      return { ok: true };
    } catch (err) {
      return { ok: false, error: (err as Error).message };
    }
  },
);

ipcMain.handle("mike:unlock", async (_e, password: unknown) => {
  if (unlocking) {
    return { ok: false, error: "Unlock already in progress." };
  }
  unlocking = true;
  try {
    if (!currentWorkspace) {
      return { ok: false, error: "No workspace selected." };
    }
    if (typeof password !== "string") {
      return { ok: false, error: "Invalid password input." };
    }
    const lock = isLockedOut(currentWorkspace);
    if (lock.locked) {
      return {
        ok: false,
        error: `Too many failed attempts. Try again in ${lock.secondsRemaining}s.`,
      };
    }
    const backendUnlockSecret = await unlockPassword(currentWorkspace, password);
    if (!backendUnlockSecret) {
      recordFailedAttempt(currentWorkspace);
      return { ok: false, error: "Incorrect password." };
    }
    recordSuccessfulAttempt(currentWorkspace);

    await startSession(currentWorkspace, backendUnlockSecret);
    spawnFrontend();
    const [backendReady, frontendReady] = await Promise.all([
      waitForBackend(20_000),
      waitForFrontend(20_000),
    ]);
    if (backendReady && sessionJwt) {
      // Load `<workspace>/.mike/secrets.enc` into the backend's
      // in-memory bundle. Failure is non-fatal: the user can still use
      // the app, they just won't have API keys configured. The UI
      // shows a "Configure API keys" panel when /internal/secrets/status
      // reports `populated: 0`.
      try {
        const result = await loadSecretsToBackend(
          currentWorkspace,
          backendUnlockSecret,
          getBackendApiBase(),
          sessionJwt,
        );
        if (result.cause === "ok") {
          console.log(
            `[secrets] loaded ${result.populated} key(s) into backend`,
          );
        } else if (result.cause === "absent") {
          console.log("[secrets] no secrets.enc yet (first run)");
        } else {
          console.warn(
            `[secrets] could not decrypt secrets.enc (cause=${result.cause}); continuing with empty bundle`,
          );
        }
      } catch (err) {
        console.warn("[secrets] load failed:", (err as Error).message);
      }
    }
    if (!backendReady) {
      // B5: surface backend startup failure instead of navigating into a
      // doomed app window.
      const exitInfo = getBackendExitInfo();
      if (exitInfo && exitInfo.code !== 0) {
        const tail = tailLogFile(50);
        dialog.showErrorBox(
          "Mike couldn't start",
          `The backend exited with code ${exitInfo.code}.\n\nLast log lines:\n\n${tail}`,
        );
        // tear down the session so the user can retry from the lock screen
        sessionJwt = null;
        sessionSecret = null;
        setBackendToken(null);
        stopFrontend();
        releaseActiveWorkspaceLock();
        if (win) loadLockScreen(win);
        return { ok: false, error: "Backend failed to start." };
      }
      console.warn("[unlock] backend slow to become ready; continuing.");
    }
    if (!frontendReady) {
      console.warn("[unlock] frontend slow to become ready; continuing.");
    }
    if (win) loadMainApp(win);
    return { ok: true };
  } catch (err) {
    const msg = (err as Error).message ?? String(err);
    console.error("[unlock] handler threw:", err);
    sessionJwt = null;
    sessionSecret = null;
    setBackendToken(null);
    stopBackend();
    stopFrontend();
    releaseActiveWorkspaceLock();
    return { ok: false, error: msg };
  } finally {
    unlocking = false;
  }
});

ipcMain.handle("mike:signOut", async () => {
  // B6: tear down the session, kill children, return to lock screen.
  sessionJwt = null;
  sessionSecret = null;
  setBackendToken(null);
  stopBackend();
  stopFrontend();
  releaseActiveWorkspaceLock();
  if (win) loadLockScreen(win);
  return { ok: true };
});

ipcMain.handle("mike:getToken", () => sessionJwt);
ipcMain.handle("mike:getUser", () => {
  if (!sessionJwt) return null;
  return { id: LOCAL_USER_ID, email: LOCAL_USER_EMAIL };
});
ipcMain.handle("mike:getApiBase", () => getBackendApiBase());

// CSP for the renderer. Allows: own scripts/styles, inline styles (Next.js
// + Tailwind ship them), images from local sources + data URIs, fetch/ws
// to the backend on localhost. Blocks: external scripts, plugins, frames,
// remote images. LLM-rendered markdown is the realistic injection vector;
// this header closes a large class of those without breaking the app.
const RENDERER_CSP = [
  "default-src 'self' http://localhost:* ws://localhost:*",
  "script-src 'self' 'unsafe-inline' 'unsafe-eval' http://localhost:*",
  "style-src 'self' 'unsafe-inline' http://localhost:*",
  "img-src 'self' data: blob: http://localhost:*",
  "font-src 'self' data: http://localhost:*",
  "connect-src 'self' http://localhost:* ws://localhost:* https://api.anthropic.com https://generativelanguage.googleapis.com",
  "frame-src 'none'",
  "object-src 'none'",
  "base-uri 'self'",
].join("; ");

function installCsp(): void {
  // CSP is enforced on packaged builds only. Next.js dev mode (Turbopack)
  // and React Fast Refresh make extra fetches to undocumented endpoints
  // that are awkward to whitelist; in dev we trust the local toolchain.
  // The packaged build serves Next.js standalone output where the URL
  // surface is fixed and the CSP can be locked down.
  if (!app.isPackaged) return;
  session.defaultSession.webRequest.onHeadersReceived((details, cb) => {
    const headers = { ...details.responseHeaders };
    // Strip any upstream CSP so ours is the one that applies.
    for (const k of Object.keys(headers)) {
      if (k.toLowerCase() === "content-security-policy") delete headers[k];
    }
    headers["Content-Security-Policy"] = [RENDERER_CSP];
    cb({ responseHeaders: headers });
  });
}

app.whenReady().then(() => {
  installCsp();
  const cfg = readConfig();
  if (isWorkspaceValid(cfg.lastWorkspace)) {
    currentWorkspace = cfg.lastWorkspace!;
    try {
      const lp = initLogging(currentWorkspace);
      console.log(`[startup] logging to ${lp}`);
    } catch (err) {
      console.warn("[startup] failed to init log file:", err);
    }
  }
  win = createWindow();
  loadLockScreen(win);
  win.on("closed", () => {
    win = null;
    lockWebContents = null;
  });
});

app.on("window-all-closed", () => {
  stopBackend();
  stopFrontend();
  releaseActiveWorkspaceLock();
  app.quit();
});

app.on("before-quit", () => {
  stopBackend();
  stopFrontend();
  releaseActiveWorkspaceLock();
  closeLogging();
});

app.on("activate", () => {
  if (BrowserWindow.getAllWindows().length === 0 && app.isReady()) {
    win = createWindow();
    loadLockScreen(win);
  }
});
