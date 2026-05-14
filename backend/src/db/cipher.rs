//! SQLCipher key derivation and pragma helpers.
//!
//! The backend never sees the user's PIN. Electron derives a 32-byte
//! root from the PIN (Argon2id), then HMAC-SHA256s labeled subkeys and
//! passes one of them (`MIKE_BACKEND_UNLOCK_SECRET`) to the backend via
//! env at spawn time. This module produces the SQLCipher key from that
//! unlock secret using the same labeled-HKDF pattern: a fresh
//! HMAC-SHA256(unlock_secret, "sqlcipher") gives us a 32-byte cipher
//! key that's distinct from the JWT key, the secrets-bundle key, and
//! every future labeled subkey.
//!
//! Why labeled HMAC instead of re-running Argon2id per purpose:
//! Argon2id is intentionally expensive (~100ms with our parameters).
//! Doing it once for the PIN, then deriving N cheap labeled subkeys, is
//! the standard HKDF pattern. See `docs/decisions.md` (HKDF-style key
//! derivation) for the architectural decision.

use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Decode a hex-encoded secret. Falls back to raw bytes for backward
/// compatibility with tests that set a plain string as the unlock
/// secret. Production Electron always passes hex.
fn decode_secret(raw: &str) -> Vec<u8> {
    hex::decode(raw).unwrap_or_else(|_| raw.as_bytes().to_vec())
}

/// Derive the SQLCipher database key. Returns 32 hex-encoded bytes
/// (64 chars) suitable for passing to `PRAGMA key = "x'<hex>'"`.
///
/// The hex form is mandatory: SQLCipher's PRAGMA key with a quoted
/// string runs the input through a key-derivation function (its own
/// PBKDF2), which would make the resulting db unportable across
/// Mike versions if the KDF parameters ever change. The `x'<hex>'`
/// form bypasses that and uses the raw bytes as the cipher key.
pub fn database_key_hex() -> Result<String> {
    let unlock_secret = std::env::var("MIKE_BACKEND_UNLOCK_SECRET")
        .map_err(|_| anyhow!("MIKE_BACKEND_UNLOCK_SECRET not set; backend must be spawned by Electron"))?;
    let secret = decode_secret(&unlock_secret);
    let mut mac = HmacSha256::new_from_slice(&secret)
        .map_err(|e| anyhow!("invalid backend unlock secret length: {e}"))?;
    mac.update(b"sqlcipher");
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// SQL fragment that applies the cipher key + sane defaults to a new
/// connection. Executed via `after_connect` so every pooled connection
/// is keyed identically.
///
/// `cipher_compatibility = 4` is the SQLCipher 4 default; we set it
/// explicitly so a database created with an older SQLCipher won't
/// silently keep its old format when read by a 4.x build.
pub fn pragma_sql(key_hex: &str) -> String {
    format!(
        "PRAGMA key = \"x'{key_hex}'\";\n\
         PRAGMA cipher_compatibility = 4;\n\
         PRAGMA cipher_memory_security = ON;\n"
    )
}

/// Translate the "file is not a database" sqlx error (which is what
/// SQLCipher returns for any operation when the key is wrong) into a
/// human-readable message.
pub fn explain_open_failure(err: &sqlx::Error) -> Option<&'static str> {
    let msg = err.to_string().to_lowercase();
    if msg.contains("file is not a database") || msg.contains("file is encrypted") {
        Some(
            "SQLCipher rejected the database key. The workspace was likely created with a \
             different PIN, or the database has been re-keyed. Verify the PIN; if it's \
             correct, the file may be corrupt — see docs/07-rebuilds.md.",
        )
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pragma_sql_uses_x_quoted_hex() {
        let sql = pragma_sql("deadbeef");
        // The exact format matters for SQLCipher: x'<hex>' bypasses
        // SQLCipher's PBKDF2 over the literal string.
        assert!(sql.contains("PRAGMA key = \"x'deadbeef'\""));
        assert!(sql.contains("cipher_compatibility = 4"));
    }

    #[test]
    fn database_key_is_32_bytes_hex() {
        // SAFETY: test process; serial single-threaded module test.
        unsafe {
            std::env::set_var(
                "MIKE_BACKEND_UNLOCK_SECRET",
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            );
        }
        let key = database_key_hex().expect("key");
        assert_eq!(key.len(), 64, "32 bytes => 64 hex chars");
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
        unsafe { std::env::remove_var("MIKE_BACKEND_UNLOCK_SECRET"); }
    }

    #[test]
    fn database_key_changes_when_unlock_secret_changes() {
        unsafe {
            std::env::set_var("MIKE_BACKEND_UNLOCK_SECRET", "00".repeat(32));
        }
        let a = database_key_hex().unwrap();
        unsafe {
            std::env::set_var("MIKE_BACKEND_UNLOCK_SECRET", "ff".repeat(32));
        }
        let b = database_key_hex().unwrap();
        assert_ne!(a, b, "different unlock secret must produce different cipher key");
        unsafe { std::env::remove_var("MIKE_BACKEND_UNLOCK_SECRET"); }
    }
}
