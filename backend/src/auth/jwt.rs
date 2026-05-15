//! JWT minting/verification. Electron is the **issuer** in Phase 1 —
//! it mints tokens with a 24h TTL right after PIN unlock. The backend
//! is the **verifier** here. The `sign_token` helper exists only for
//! tests; production paths never use it.

use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// Test-only TTL. Production tokens are minted by Electron with a
/// 24-hour TTL (electron/main.ts: JWT_TTL_SECONDS). This constant
/// only matters when a test calls `sign_token` directly — long
/// enough that test runs never expire under it.
const TEST_TOKEN_EXPIRY_SECS: u64 = 60 * 60 * 24 * 30;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,   // user_id
    pub email: String,
    pub exp: u64,
    pub iat: u64,
}

type HmacSha256 = Hmac<Sha256>;

fn decode_secret(raw: &str) -> Vec<u8> {
    hex::decode(raw).unwrap_or_else(|_| raw.as_bytes().to_vec())
}

fn derive_labeled_key(secret: &[u8], label: &str) -> Result<Vec<u8>> {
    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|e| anyhow!("invalid backend unlock secret: {e}"))?;
    mac.update(label.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn secret_bytes() -> Result<Vec<u8>> {
    if let Ok(jwt_secret) = std::env::var("MIKE_JWT_SECRET").or_else(|_| std::env::var("JWT_SECRET")) {
        return Ok(decode_secret(&jwt_secret));
    }
    let unlock_secret = std::env::var("MIKE_BACKEND_UNLOCK_SECRET")
        .map_err(|_| anyhow!("MIKE_BACKEND_UNLOCK_SECRET not set"))?;
    derive_labeled_key(&decode_secret(&unlock_secret), "jwt-verification")
}

pub fn ensure_configured() -> Result<()> {
    secret_bytes().map(|_| ())
}

/// **Test-only.** Production tokens are minted by Electron's
/// `signLocalJwt` (electron/jwt.ts) with a 24h TTL. The backend
/// only verifies. Any production code path that wants to call this
/// is doing the wrong thing.
pub fn sign_token(user_id: &str, email: &str) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let claims = Claims {
        sub: user_id.to_string(),
        email: email.to_string(),
        iat: now,
        exp: now + TEST_TOKEN_EXPIRY_SECS,
    };
    let secret = secret_bytes()?;
    let key = EncodingKey::from_secret(&secret);
    Ok(encode(&Header::new(Algorithm::HS256), &claims, &key)?)
}

pub fn verify_token(token: &str) -> Result<Claims> {
    let secret = secret_bytes()?;
    let key = DecodingKey::from_secret(&secret);
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    let data = decode::<Claims>(token, &key, &validation)
        .map_err(|e| anyhow!("Invalid token: {e}"))?;
    Ok(data.claims)
}
