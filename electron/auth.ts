import * as fs from "fs";
import * as crypto from "crypto";
import { argon2id } from "hash-wasm";
import {
  authFilePath,
  authStateFilePath,
  atomicWriteFileSync,
} from "./workspace";

interface AuthFile {
  version: 1;
  algo: "argon2id";
  salt: string;
  verifier: string;
  iterations: number;
  parallelism: number;
  memorySize: number;
  hashLength: number;
}

interface AuthState {
  version: 1;
  failedAttempts: number;
  lockoutUntil: number;
}

const MIN_PASSWORD_LENGTH = 4;
const ARGON2ID_PARAMS = {
  iterations: 3,
  parallelism: 1,
  memorySize: 64 * 1024,
  hashLength: 64,
} as const;

const FAILED_ATTEMPT_LIMIT = 5;
const LOCKOUT_MS = 30_000;

async function derivePinRoot(
  pin: string,
  saltB64: string,
  params: Pick<AuthFile, "iterations" | "parallelism" | "memorySize" | "hashLength"> = ARGON2ID_PARAMS,
): Promise<Buffer> {
  const salt = Buffer.from(saltB64, "base64");
  const root = await argon2id({
    password: pin,
    salt,
    iterations: params.iterations,
    parallelism: params.parallelism,
    memorySize: params.memorySize,
    hashLength: params.hashLength,
    outputType: "binary",
  });
  return Buffer.from(root);
}

function labeledSecretHex(root: Buffer, label: string): string {
  return crypto.createHmac("sha256", root).update(label).digest("hex");
}

function pinVerifierHex(root: Buffer): string {
  return labeledSecretHex(root, "pin-verifier");
}

function backendUnlockSecretHex(root: Buffer): string {
  return labeledSecretHex(root, "backend-unlock");
}

export function hasPassword(workspace: string): boolean {
  try {
    return fs.statSync(authFilePath(workspace)).isFile();
  } catch {
    return false;
  }
}

export const PASSWORD_MIN_LENGTH = MIN_PASSWORD_LENGTH;

export async function setPassword(
  workspace: string,
  password: string,
): Promise<void> {
  if (!/^\d{4,8}$/.test(password)) {
    throw new Error("PIN must be 4-8 digits.");
  }
  const salt = crypto.randomBytes(16);
  const root = await derivePinRoot(password, salt.toString("base64"));
  const file: AuthFile = {
    version: 1,
    algo: "argon2id",
    salt: salt.toString("base64"),
    verifier: pinVerifierHex(root),
    ...ARGON2ID_PARAMS,
  };
  atomicWriteFileSync(
    authFilePath(workspace),
    JSON.stringify(file, null, 2),
    { mode: 0o600 },
  );
}

/**
 * Verify the PIN and return the single backend unlock secret Electron passes
 * to the Rust child. The stored file contains only a verifier derived from the
 * Argon2id root, not the backend secret itself.
 */
export async function unlockPassword(
  workspace: string,
  password: string,
): Promise<string | null> {
  let raw: string;
  try {
    raw = fs.readFileSync(authFilePath(workspace), "utf8");
  } catch {
    return null;
  }
  let file: AuthFile;
  try {
    file = JSON.parse(raw) as AuthFile;
  } catch {
    return null;
  }
  if (file.algo !== "argon2id") return null;
  const root = await derivePinRoot(password, file.salt, {
    iterations: file.iterations,
    parallelism: file.parallelism,
    memorySize: file.memorySize,
    hashLength: file.hashLength,
  });
  const candidate = Buffer.from(pinVerifierHex(root), "hex");
  const stored = Buffer.from(file.verifier, "hex");
  if (candidate.length !== stored.length) return null;
  if (!crypto.timingSafeEqual(candidate, stored)) return null;
  return backendUnlockSecretHex(root);
}

export async function verifyPassword(
  workspace: string,
  password: string,
): Promise<boolean> {
  return (await unlockPassword(workspace, password)) !== null;
}

export async function changePassword(
  workspace: string,
  oldPassword: string,
  newPassword: string,
): Promise<boolean> {
  const secret = await unlockPassword(workspace, oldPassword);
  if (!secret) {
    return false;
  }
  await setPassword(workspace, newPassword);
  return true;
}

// ---------------------------------------------------------------------------
// Lockout state — persisted per workspace
// ---------------------------------------------------------------------------

function readState(workspace: string): AuthState {
  try {
    const raw = fs.readFileSync(authStateFilePath(workspace), "utf8");
    const parsed = JSON.parse(raw) as Partial<AuthState>;
    return {
      version: 1,
      failedAttempts: Number(parsed.failedAttempts ?? 0) || 0,
      lockoutUntil: Number(parsed.lockoutUntil ?? 0) || 0,
    };
  } catch {
    return { version: 1, failedAttempts: 0, lockoutUntil: 0 };
  }
}

function writeState(workspace: string, state: AuthState): void {
  try {
    atomicWriteFileSync(
      authStateFilePath(workspace),
      JSON.stringify(state, null, 2),
      { mode: 0o600 },
    );
  } catch (err) {
    console.warn("[auth] failed to persist auth-state:", err);
  }
}

export function isLockedOut(workspace: string): {
  locked: boolean;
  secondsRemaining: number;
} {
  const state = readState(workspace);
  const now = Date.now();
  if (now < state.lockoutUntil) {
    return {
      locked: true,
      secondsRemaining: Math.ceil((state.lockoutUntil - now) / 1000),
    };
  }
  return { locked: false, secondsRemaining: 0 };
}

export function recordFailedAttempt(workspace: string): void {
  const state = readState(workspace);
  state.failedAttempts++;
  if (state.failedAttempts >= FAILED_ATTEMPT_LIMIT) {
    state.lockoutUntil = Date.now() + LOCKOUT_MS;
    state.failedAttempts = 0;
  }
  writeState(workspace, state);
}

export function recordSuccessfulAttempt(workspace: string): void {
  writeState(workspace, {
    version: 1,
    failedAttempts: 0,
    lockoutUntil: 0,
  });
}
