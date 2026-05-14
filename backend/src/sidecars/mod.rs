//! Sidecar trait + supervisor. The Rust-side membrane in front of
//! the Python sidecars (Docling today; eyecite, presidio later).
//!
//! Phase split (PLAN.md):
//!   - Phase 1 (current): Electron spawns Python; this module READS
//!     the runtime file Electron wrote, health-checks, surfaces
//!     supervisor state to routes. It does NOT spawn. The
//!     [`Sidecar::spawn`] hook is a placeholder that returns
//!     `SpawnDelegatedToElectron`.
//!   - Phase 3: this module gains a real spawn implementation using
//!     `tokio::process::Command`, restart-with-backoff, and
//!     version-check; the Electron-side `docling.ts` collapses to a
//!     thin "Python found?" helper.
//!
//! Anti-pattern #6 says the backend is the membrane: routes never
//! call sidecars directly, they go through the supervisor's typed
//! clients. Even at Phase 1 (when Electron is still doing the
//! spawning), the *interface* lives here so that routes/documents.rs
//! and friends can be written as if the supervisor were already
//! Rust-side. When Phase 3 lands, only the implementation behind this
//! trait changes; no callers move.
//!
//! See docs/03-sidecars.md for the wire-level contract.

pub mod docling;
pub mod supervisor;

use std::collections::HashMap;

/// Per-sidecar concurrency policy. Implementations express how many
/// uvicorn workers they want; the supervisor reads this to decide
/// `--workers N` (or, in the Electron-managed Phase 1 path, passes
/// it as `MIKE_SIDECAR_WORKERS` for the sidecar to read at startup).
#[derive(Debug, Clone, Copy)]
pub enum SidecarConcurrency {
    /// One worker; safe for non-thread-safe model loaders.
    #[allow(dead_code)]
    SingleWorker,
    /// N workers; each forks its own model copy. Memory = N × model.
    MultiWorker { default: u32, max: u32 },
}

#[async_trait::async_trait]
pub trait Sidecar: Send + Sync + 'static {
    /// Wire-name (`"docling"`, `"eyecite"`, …). Used for the runtime
    /// file path and log file path.
    fn name(&self) -> &'static str;

    /// Major version of the sidecar's API this build expects. The
    /// supervisor refuses to use a sidecar whose `/version` reports a
    /// different major.
    fn expected_major_version(&self) -> u32;

    /// Default + maximum worker count.
    fn concurrency(&self) -> SidecarConcurrency;

    /// Additional env vars to pass to the sidecar at spawn. Used for
    /// sidecar-specific config (e.g. Docling's MIKE_DOCLING_DEVICE).
    /// The supervisor merges these with the universal `MIKE_SIDECAR_*`
    /// envelope per docs/03-sidecars.md.
    fn extra_env(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

/// What the supervisor knows about a sidecar right now.
#[derive(Debug, Clone, PartialEq)]
pub enum SupervisorState {
    /// `/health` + `/version` both responded; major version matches.
    Healthy { port: u16, version: String },
    /// Sidecar is running but not usable (version mismatch, /health
    /// failing). Routes that need it should return 503 with
    /// `X-Sidecar-Required: <name>@<expected_major>`.
    Degraded { reason: String },
    /// No sidecar process; runtime file missing or stale.
    Down,
}

/// Build the 503 response routes return when a required sidecar
/// is unavailable. This is the chokepoint that enforces anti-pattern
/// #7 (no silent fallbacks): every code path that needs a sidecar
/// goes through this function rather than picking a worse extractor.
///
/// Example use in a route:
/// ```ignore
/// match state.sidecars.state("docling").await {
///     SupervisorState::Healthy { .. } => parse_with_docling(...).await,
///     other => return Err(sidecar_unavailable_response("docling", 1, &other)),
/// }
/// ```
pub fn sidecar_unavailable_response(
    name: &'static str,
    expected_major: u32,
    state: &SupervisorState,
) -> axum::response::Response {
    use axum::http::{HeaderValue, StatusCode};
    use axum::response::IntoResponse;

    let reason = state
        .reason()
        .unwrap_or("unknown")
        .to_string();
    let body = serde_json::json!({
        "error": format!("Sidecar '{name}' is unavailable: {reason}"),
        "sidecar": name,
        "expected_major": expected_major,
    });
    let mut resp = (StatusCode::SERVICE_UNAVAILABLE, axum::Json(body)).into_response();
    let header = format!("{name}@{expected_major}");
    if let Ok(v) = HeaderValue::from_str(&header) {
        resp.headers_mut().insert("X-Sidecar-Required", v);
    }
    resp.headers_mut()
        .insert("Retry-After", HeaderValue::from_static("60"));
    resp
}

impl SupervisorState {
    pub fn is_healthy(&self) -> bool {
        matches!(self, SupervisorState::Healthy { .. })
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            SupervisorState::Degraded { reason } => Some(reason.as_str()),
            SupervisorState::Down => Some("not running"),
            SupervisorState::Healthy { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervisor_state_classification() {
        assert!(SupervisorState::Healthy { port: 1, version: "1.0.0".into() }.is_healthy());
        assert!(!SupervisorState::Down.is_healthy());
        assert!(!SupervisorState::Degraded { reason: "x".into() }.is_healthy());
        assert_eq!(SupervisorState::Down.reason(), Some("not running"));
    }
}
