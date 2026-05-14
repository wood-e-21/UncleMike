import * as crypto from "crypto";

/**
 * HKDF-style labeled key derivation.
 *
 * The PIN goes through Argon2id once (in auth.ts) to produce a 64-byte
 * root. From that root we mint as many distinct purpose-specific keys
 * as we need by HMAC-SHA256(root, label). The labels MUST match what
 * the backend uses, otherwise the keys won't agree.
 *
 * Current label inventory (keep this in sync with backend code):
 *   - "pin-verifier"       (electron/auth.ts)             → stored in pin.json
 *   - "backend-unlock"     (electron/auth.ts)             → MIKE_BACKEND_UNLOCK_SECRET
 *   - "jwt-verification"   (this file + backend/auth/jwt) → JWT signing/verification
 *   - "secrets-bundle"     (electron/secrets.ts)          → secrets.enc AES-GCM key
 *   - "sqlcipher"          (backend/db/cipher.rs)         → SQLCipher database key
 *
 * See docs/decisions.md for the architectural rationale.
 */
export function deriveLabeledHex(secretHex: string, label: string): string {
  return crypto
    .createHmac("sha256", Buffer.from(secretHex, "hex"))
    .update(label)
    .digest("hex");
}

export function deriveJwtSecretHex(backendUnlockSecretHex: string): string {
  return deriveLabeledHex(backendUnlockSecretHex, "jwt-verification");
}

export function deriveSecretsBundleKeyHex(backendUnlockSecretHex: string): string {
  return deriveLabeledHex(backendUnlockSecretHex, "secrets-bundle");
}
