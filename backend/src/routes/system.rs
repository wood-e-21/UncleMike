//! System-level status route.
//!
//! `GET /system/status` returns the supervisor's view of every
//! registered sidecar. The frontend polls this to render the
//! "degraded" banner when a sidecar is down or version-mismatched.
//!
//! This is the read side of the no-silent-fallback rule
//! (anti-pattern #7). Routes that need a sidecar return 503 with
//! `X-Sidecar-Required: <name>@<expected_major>`; the frontend
//! reads /system/status to explain *which* sidecar and *why*.

use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::middleware::AuthUser, sidecars::SupervisorState, AppState};

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/status", get(status))
}

async fn status(State(state): State<Arc<AppState>>, _auth: AuthUser) -> Json<Value> {
    // Today: one sidecar (docling). Loop trivially extends when we
    // add eyecite / presidio.
    let docling = state.sidecars.state("docling").await;
    let docling_json = match &docling {
        SupervisorState::Healthy { port, version } => json!({
            "status": "healthy",
            "port": port,
            "version": version,
        }),
        SupervisorState::Degraded { reason } => json!({
            "status": "degraded",
            "reason": reason,
        }),
        SupervisorState::Down => json!({ "status": "down" }),
    };

    Json(json!({
        "sidecars": { "docling": docling_json },
        // Backend itself is healthy by definition if it's serving
        // this request. Future fields: workspace lock holder, DB
        // schema version, etc.
        "backend": { "status": "healthy" },
    }))
}
