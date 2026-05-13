import * as crypto from "crypto";

export function deriveLabeledHex(secretHex: string, label: string): string {
  return crypto
    .createHmac("sha256", Buffer.from(secretHex, "hex"))
    .update(label)
    .digest("hex");
}

export function deriveJwtSecretHex(backendUnlockSecretHex: string): string {
  return deriveLabeledHex(backendUnlockSecretHex, "jwt-verification");
}
