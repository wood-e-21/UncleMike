use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const EXPIRY_SECS: u64 = 60 * 60 * 24 * 30; // 30 days

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

pub fn sign_token(user_id: &str, email: &str) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let claims = Claims {
        sub: user_id.to_string(),
        email: email.to_string(),
        iat: now,
        exp: now + EXPIRY_SECS,
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
