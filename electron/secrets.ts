import * as crypto from "crypto";
import * as fs from "fs";
import { secretsFilePath, atomicWriteFileSync } from "./workspace";
import { deriveLabeledHex } from "./keys";

/**
 * Encrypted secrets bundle on disk at `<workspace>/.mike/secrets.enc`.
 *
 * Format (binary):
 *   bytes 0..2     "MS" + version byte (currently 0x01)
 *   bytes 3..15    12-byte AES-GCM nonce (random per write)
 *   bytes 15..end  AES-256-GCM ciphertext + 16-byte auth tag
 *
 * The encryption key is derived from the same Argon2id-rooted unlock
 * secret that produces the SQLCipher key and the JWT signing key:
 *   key = HMAC-SHA256(unlock_secret_hex, "secrets-bundle")
 *
 * This means:
 *   - One PIN unlocks everything
 *   - The same secrets.enc file is portable across machines (any
 *     machine that can derive the unlock secret from the same PIN can
 *     decrypt). Backup-friendly.
 *   - No dependency on Electron's safeStorage / OS keychain (the
 *     prior Phase-1-reshape implementation used it; see
 *     `docs/decisions.md` for why we moved off).
 */

export interface SecretsBundle {
  anthropic_api_key?: string;
  gemini_api_key?: string;
  openrouter_api_key?: string;
  openai_api_key?: string;
  resend_api_key?: string;
}

export interface SecretsReadResult {
  secrets: SecretsBundle;
  /**
   * `absent` — no secrets.enc file yet.
   * `decrypt_failed` — file exists but the AEAD tag rejected the key
   *   (different PIN, or the file is corrupted).
   * `parse_failed` — decrypted bytes weren't valid JSON.
   * `ok` — bundle loaded.
   */
  cause: "ok" | "absent" | "decrypt_failed" | "parse_failed";
}

const FORMAT_MAGIC = Buffer.from([0x4d, 0x53]); // "MS"
const FORMAT_VERSION = 0x01;
const NONCE_LEN = 12;
const TAG_LEN = 16;

function deriveSecretsKey(unlockSecretHex: string): Buffer {
  return Buffer.from(deriveLabeledHex(unlockSecretHex, "secrets-bundle"), "hex");
}

export function encryptSecrets(
  secrets: SecretsBundle,
  unlockSecretHex: string,
): Buffer {
  const key = deriveSecretsKey(unlockSecretHex);
  const nonce = crypto.randomBytes(NONCE_LEN);
  const cipher = crypto.createCipheriv("aes-256-gcm", key, nonce);
  const plaintext = Buffer.from(JSON.stringify(secrets), "utf8");
  const ciphertext = Buffer.concat([cipher.update(plaintext), cipher.final()]);
  const tag = cipher.getAuthTag();
  return Buffer.concat([
    FORMAT_MAGIC,
    Buffer.from([FORMAT_VERSION]),
    nonce,
    ciphertext,
    tag,
  ]);
}

export function decryptSecrets(
  blob: Buffer,
  unlockSecretHex: string,
): SecretsBundle {
  if (blob.length < FORMAT_MAGIC.length + 1 + NONCE_LEN + TAG_LEN) {
    throw new Error("secrets.enc too short to be valid");
  }
  if (!blob.subarray(0, FORMAT_MAGIC.length).equals(FORMAT_MAGIC)) {
    throw new Error("secrets.enc magic bytes mismatch");
  }
  const version = blob[FORMAT_MAGIC.length];
  if (version !== FORMAT_VERSION) {
    throw new Error(`secrets.enc version ${version} not supported`);
  }
  const headerLen = FORMAT_MAGIC.length + 1;
  const nonce = blob.subarray(headerLen, headerLen + NONCE_LEN);
  const tag = blob.subarray(blob.length - TAG_LEN);
  const ciphertext = blob.subarray(headerLen + NONCE_LEN, blob.length - TAG_LEN);

  const key = deriveSecretsKey(unlockSecretHex);
  const decipher = crypto.createDecipheriv("aes-256-gcm", key, nonce);
  decipher.setAuthTag(tag);
  const plaintext = Buffer.concat([
    decipher.update(ciphertext),
    decipher.final(),
  ]);
  return JSON.parse(plaintext.toString("utf8")) as SecretsBundle;
}

export function readSecretsDetailed(
  workspace: string,
  unlockSecretHex: string,
): SecretsReadResult {
  let buf: Buffer;
  try {
    buf = fs.readFileSync(secretsFilePath(workspace));
  } catch {
    return { secrets: {}, cause: "absent" };
  }
  let bundle: SecretsBundle;
  try {
    bundle = decryptSecrets(buf, unlockSecretHex);
  } catch (err) {
    const msg = (err as Error).message ?? String(err);
    if (msg.includes("Unsupported state") || msg.includes("auth tag")) {
      return { secrets: {}, cause: "decrypt_failed" };
    }
    if (msg.includes("JSON")) {
      return { secrets: {}, cause: "parse_failed" };
    }
    console.warn("[secrets] decryption failed:", msg);
    return { secrets: {}, cause: "decrypt_failed" };
  }
  return { secrets: bundle, cause: "ok" };
}

/** Convenience wrapper preserving the old surface. */
export function readSecrets(
  workspace: string,
  unlockSecretHex: string,
): SecretsBundle {
  return readSecretsDetailed(workspace, unlockSecretHex).secrets;
}

export function writeSecrets(
  workspace: string,
  secrets: SecretsBundle,
  unlockSecretHex: string,
): void {
  const blob = encryptSecrets(secrets, unlockSecretHex);
  atomicWriteFileSync(secretsFilePath(workspace), blob, { mode: 0o600 });
}

export function hasSecrets(workspace: string): boolean {
  try {
    return fs.statSync(secretsFilePath(workspace)).isFile();
  } catch {
    return false;
  }
}

/**
 * Read the on-disk bundle (if present) and POST it to the backend's
 * `/internal/secrets/load` endpoint. Called once per session, after
 * the backend's `READY` token + before the main app loads.
 *
 * Failure mode: if there's no file or it can't be decrypted, we POST
 * an empty bundle. The backend treats that as "no API keys configured"
 * — UI shows the "Configure API keys" panel.
 */
export async function loadSecretsToBackend(
  workspace: string,
  unlockSecretHex: string,
  apiBase: string,
  jwt: string,
): Promise<{ ok: boolean; populated: number; cause: SecretsReadResult["cause"] }> {
  const { secrets, cause } = readSecretsDetailed(workspace, unlockSecretHex);
  const resp = await fetch(`${apiBase}/internal/secrets/load`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${jwt}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify(secrets),
  });
  if (!resp.ok) {
    return { ok: false, populated: 0, cause };
  }
  const body = (await resp.json()) as { populated?: number };
  return { ok: true, populated: body.populated ?? 0, cause };
}
