//! Internal routes — only Electron talks to these. Same auth as
//! every other route (bearer JWT signed by Electron) plus Host-header
//! middleware in `lib.rs`.
//!
//! The convention `/internal/*` is descriptive, not security. Anyone
//! with a valid JWT can hit these. The threat model relies on
//! 127.0.0.1-only binding + Host check to keep external callers out.
//!
//! Today: secrets load/save.
//! Future: `POST /internal/shutdown` for clean Electron-initiated
//! teardown; `POST /internal/audit/event` if anything wants to write
//! to the audit log without going through a domain route.

use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{audit, auth::middleware::AuthUser, secrets::SecretsBundle, AppState};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/secrets/load", post(load_secrets))
        .route("/secrets/clear", post(clear_secrets))
        .route("/secrets/status", get(secrets_status))
}

/// Replace the in-memory secrets bundle. Body is the plaintext JSON
/// that Electron just decrypted from `secrets.enc`.
///
/// Returns the number of populated keys (NOT the keys themselves).
/// Electron uses this for a startup-time log line: "loaded 3 API keys
/// into backend." That count is the only thing about secrets that
/// ever appears in any log on either side.
async fn load_secrets(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Json(body): Json<SecretsBundle>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut guard = state.secrets.write().await;
    let count = guard.populate(body);
    drop(guard);
    tracing::info!("[secrets] bundle loaded with {count} populated key(s)");
    audit::log(
        &state.paths,
        audit::AuditEvent::SecretsLoaded { populated: count },
    );
    Ok(Json(json!({ "ok": true, "populated": count })))
}

async fn clear_secrets(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
) -> Json<Value> {
    state.secrets.write().await.clear();
    tracing::info!("[secrets] bundle cleared");
    audit::log(&state.paths, audit::AuditEvent::SecretsCleared);
    Json(json!({ "ok": true }))
}

/// Read-only: count of populated keys. No key names, no values. Used
/// by the frontend's account/models screen to show "API keys
/// configured: 3" without ever surfacing the keys themselves.
async fn secrets_status(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
) -> Json<Value> {
    let guard = state.secrets.read().await;
    Json(json!({
        "populated": guard.populated_count(),
        "has_anthropic": guard.anthropic_api_key.as_ref().is_some_and(|s| !s.trim().is_empty()),
        "has_gemini": guard.gemini_api_key.as_ref().is_some_and(|s| !s.trim().is_empty()),
        "has_openrouter": guard.openrouter_api_key.as_ref().is_some_and(|s| !s.trim().is_empty()),
        "has_openai": guard.openai_api_key.as_ref().is_some_and(|s| !s.trim().is_empty()),
    }))
}
