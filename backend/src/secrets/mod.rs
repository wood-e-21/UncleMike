//! In-process secrets bundle. The Rust side of the keystore the
//! Electron shell maintains in `<workspace>/.mike/secrets.enc`.
//!
//! Flow:
//!   1. Electron decrypts `secrets.enc` (AES-256-GCM, key derived from
//!      the same Argon2id root as the SQLCipher / JWT keys).
//!   2. Electron POSTs the plaintext JSON to `POST /internal/secrets/load`
//!      on this backend.
//!   3. We hold the bundle in `AppState.secrets`. Memory-only — never
//!      written to disk, never re-emitted to environment, never logged.
//!   4. LLM modules read from `state.secrets` to fetch API keys.
//!
//! Memory hygiene: the bundle is wrapped in `Arc<RwLock<SecretsBundle>>`.
//! When the user signs out, Electron tears down the backend process,
//! so we don't need explicit zeroization for the v1 threat model
//! (state-level adversaries with cold-boot capability are out of
//! scope; see docs/08-security-model.md).

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretsBundle {
    pub anthropic_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub resend_api_key: Option<String>,
}

impl SecretsBundle {
    /// Replace the in-memory bundle with a freshly-loaded one. Used
    /// by `POST /internal/secrets/load`. Returns a count of the
    /// non-empty keys so the route can log without leaking values.
    pub fn populate(&mut self, fresh: SecretsBundle) -> usize {
        *self = fresh;
        self.populated_count()
    }

    pub fn populated_count(&self) -> usize {
        let mut n = 0;
        for v in [
            &self.anthropic_api_key,
            &self.gemini_api_key,
            &self.openrouter_api_key,
            &self.openai_api_key,
            &self.resend_api_key,
        ] {
            if v.as_ref().is_some_and(|s| !s.trim().is_empty()) {
                n += 1;
            }
        }
        n
    }

    pub fn clear(&mut self) {
        *self = SecretsBundle::default();
    }
}

pub type SharedSecrets = Arc<RwLock<SecretsBundle>>;

pub fn new_shared() -> SharedSecrets {
    Arc::new(RwLock::new(SecretsBundle::default()))
}

/// Resolve an Anthropic API key for the current request. Precedence:
///   1. The in-memory bundle loaded from `secrets.enc` via Electron.
///   2. The legacy `user_settings.claude_api_key` column passed in by
///      the route layer as `legacy_fallback`. Will be removed when the
///      UI is migrated to write through `/internal/secrets/save`.
///   3. None — caller surfaces a "not configured" error.
///
/// **Environment variables are not consulted.** Anti-pattern #9 says
/// no env-var-passed secrets after backend startup; this helper is the
/// chokepoint that enforces it for LLM modules.
pub async fn anthropic_key(
    secrets: &SharedSecrets,
    legacy_fallback: Option<&str>,
) -> Option<String> {
    if let Some(s) = secrets
        .read()
        .await
        .anthropic_api_key
        .as_ref()
        .filter(|s| !s.trim().is_empty())
    {
        return Some(s.clone());
    }
    legacy_fallback
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

pub async fn gemini_key(
    secrets: &SharedSecrets,
    legacy_fallback: Option<&str>,
) -> Option<String> {
    if let Some(s) = secrets
        .read()
        .await
        .gemini_api_key
        .as_ref()
        .filter(|s| !s.trim().is_empty())
    {
        return Some(s.clone());
    }
    legacy_fallback
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

pub async fn openrouter_key(
    secrets: &SharedSecrets,
    legacy_fallback: Option<&str>,
) -> Option<String> {
    if let Some(s) = secrets
        .read()
        .await
        .openrouter_api_key
        .as_ref()
        .filter(|s| !s.trim().is_empty())
    {
        return Some(s.clone());
    }
    legacy_fallback
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn populated_count_ignores_empty_strings() {
        let mut b = SecretsBundle::default();
        assert_eq!(b.populated_count(), 0);
        b.anthropic_api_key = Some(String::new());
        assert_eq!(b.populated_count(), 0);
        b.anthropic_api_key = Some("sk-real-key".into());
        assert_eq!(b.populated_count(), 1);
    }

    #[tokio::test]
    async fn bundle_wins_over_legacy_fallback() {
        let shared = new_shared();
        shared.write().await.anthropic_api_key = Some("from-bundle".into());
        let resolved = anthropic_key(&shared, Some("from-legacy")).await;
        assert_eq!(resolved.as_deref(), Some("from-bundle"));
    }

    #[tokio::test]
    async fn empty_bundle_falls_through_to_legacy() {
        let shared = new_shared();
        // Bundle present but its anthropic key is blank.
        shared.write().await.anthropic_api_key = Some("   ".into());
        let resolved = anthropic_key(&shared, Some("from-legacy")).await;
        assert_eq!(resolved.as_deref(), Some("from-legacy"));
    }

    #[tokio::test]
    async fn missing_bundle_falls_through_to_legacy() {
        let shared = new_shared();
        let resolved = anthropic_key(&shared, Some("from-legacy")).await;
        assert_eq!(resolved.as_deref(), Some("from-legacy"));
    }

    #[tokio::test]
    async fn returns_none_when_neither_present() {
        let shared = new_shared();
        assert!(anthropic_key(&shared, None).await.is_none());
        assert!(anthropic_key(&shared, Some("")).await.is_none());
    }
}
