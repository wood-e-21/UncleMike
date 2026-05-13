import { safeStorage } from "electron";
import * as fs from "fs";
import { secretsFilePath, atomicWriteFileSync } from "./workspace";

export interface SecretsBundle {
  ANTHROPIC_API_KEY?: string;
  GEMINI_API_KEY?: string;
  OPENROUTER_API_KEY?: string;
  RESEND_API_KEY?: string;
}

export interface SecretsReadResult {
  secrets: SecretsBundle;
  /**
   * Distinguishes "no secrets file yet" (cause=`absent`) from
   * "file exists but couldn't decrypt" (cause=`decrypt_failed`) — the
   * latter usually means the OS keychain key changed (e.g. user logged in
   * under a different Windows account on the same machine).
   */
  cause: "ok" | "absent" | "encryption_unavailable" | "decrypt_failed";
}

export function readSecretsDetailed(workspace: string): SecretsReadResult {
  if (!safeStorage.isEncryptionAvailable()) {
    return { secrets: {}, cause: "encryption_unavailable" };
  }
  let buf: Buffer;
  try {
    buf = fs.readFileSync(secretsFilePath(workspace));
  } catch {
    return { secrets: {}, cause: "absent" };
  }
  try {
    const json = safeStorage.decryptString(buf);
    return { secrets: JSON.parse(json) as SecretsBundle, cause: "ok" };
  } catch (err) {
    console.warn("[secrets] decryption failed:", (err as Error).message);
    return { secrets: {}, cause: "decrypt_failed" };
  }
}

/** Convenience wrapper preserving the old surface. */
export function readSecrets(workspace: string): SecretsBundle {
  return readSecretsDetailed(workspace).secrets;
}

export function writeSecrets(
  workspace: string,
  secrets: SecretsBundle,
): void {
  if (!safeStorage.isEncryptionAvailable()) {
    throw new Error(
      "OS-level secret storage is unavailable on this machine. Cannot save API keys.",
    );
  }
  const encrypted = safeStorage.encryptString(JSON.stringify(secrets));
  atomicWriteFileSync(secretsFilePath(workspace), encrypted, { mode: 0o600 });
}

export function hasSecrets(workspace: string): boolean {
  try {
    return fs.statSync(secretsFilePath(workspace)).isFile();
  } catch {
    return false;
  }
}
