pub mod audit;
pub mod auth;
pub mod db;
pub mod embeddings;
pub mod llm;
pub mod mcp;
pub mod mikeprj;
pub mod pdf;
pub mod routes;
pub mod secrets;
pub mod sidecars;
pub mod storage;
pub mod sync;
pub mod workspace;

pub use db::AppState;

use axum::{
    extract::Request,
    http::{HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::request_id::{
    MakeRequestId, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};

pub async fn run_server(port: u16) -> anyhow::Result<()> {
    run_server_inner(port).await
}

/// Load `.env` from a known-good location regardless of cwd.
///
/// Only consulted when the backend is started standalone (not under
/// Electron supervision). When `MIKE_BACKEND_UNLOCK_SECRET` is set,
/// Electron is the parent and we treat env vars as authoritative —
/// reading an unrelated `.env` at that point would be a #9
/// anti-pattern violation (env-var-passed secrets after startup).
/// Anything Electron wants to inject, it has already injected via
/// `safeEnv()`; anything else is at best noise and at worst a footgun
/// (a stale `.env` masking a missing secrets-bundle load).
fn load_dotenv() {
    if std::env::var("MIKE_BACKEND_UNLOCK_SECRET").is_ok() {
        tracing::debug!(
            "[env] backend running under Electron supervision; skipping .env walk"
        );
        return;
    }

    fn try_walk_up(start: std::path::PathBuf) -> bool {
        let mut current: Option<std::path::PathBuf> = Some(start);
        while let Some(dir) = current {
            let candidate = dir.join(".env");
            if candidate.is_file() {
                if dotenvy::from_path(&candidate).is_ok() {
                    tracing::info!("[env] loaded {}", candidate.display());
                    return true;
                }
            }
            current = dir.parent().map(|p| p.to_path_buf());
        }
        false
    }

    if let Ok(cwd) = std::env::current_dir() {
        if try_walk_up(cwd) {
            return;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            try_walk_up(parent.to_path_buf());
        }
    }
}

/// Pin fastembed's model cache to a stable directory **outside the
/// workspace**, otherwise the ~280MB of `.part` chunks downloaded on first run
/// can land under a watched app directory and trigger file watchers repeatedly.
///
/// Honours `FASTEMBED_CACHE_DIR` if the user already set it in `.env`;
/// otherwise points at `<userdata>/mikerust-data/fastembed`. Either
/// way the directory is created so fastembed doesn't fail on first
/// `try_new`.
///
/// Called from `run_server_with_bio_tx` immediately after `load_dotenv`,
/// so the override takes effect before the embedding service spins up.
fn ensure_fastembed_cache_dir() {
    if std::env::var("FASTEMBED_CACHE_DIR").is_ok() {
        return;
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(home)
        .join("mikerust-data")
        .join("fastembed");
    let _ = std::fs::create_dir_all(&path);
    // SAFETY: single-threaded process startup before the runtime spins
    // up — no concurrent reads of std::env to race with.
    unsafe {
        std::env::set_var("FASTEMBED_CACHE_DIR", &path);
    }
    tracing::info!("[rag] fastembed cache pinned to {}", path.display());
}

async fn run_server_inner(port: u16) -> anyhow::Result<()> {
    load_dotenv();
    ensure_fastembed_cache_dir();
    auth::jwt::ensure_configured()?;

    let state = AppState::new().await?;
    let state = Arc::new(state);
    state.run_migrations().await?;

    // Startup recovery: any document still flagged as `syncing` from a
    // previous session can't actually be in flight any more — there's
    // no embedding task running for it. Flip those rows to
    // `interrupted` so the UI surfaces the resync button instead of
    // leaving them stuck with a spinner that never moves.
    let recovered = sqlx::query(
        "UPDATE documents SET status = 'interrupted' WHERE status = 'syncing'",
    )
    .execute(&state.db)
    .await
    .map(|r| r.rows_affected())
    .unwrap_or(0);
    if recovered > 0 {
        tracing::info!(
            "[startup] recovered {recovered} doc(s) from stale 'syncing' state \
             → marked 'interrupted' (resync from the UI when ready)"
        );
    }

    // Strict per-port CORS allowlist per docs/08-security-model.md
    // Decision 6. The frontend lives at http://localhost:3000 (the
    // packaged Next.js standalone server) and the Electron renderer
    // also reaches us via that origin. When the Word add-in port
    // (HTTPS:3002) lands in Phase 5 it gets its own router with its
    // own CORS allowlist.
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list([
            HeaderValue::from_static("http://localhost:3000"),
            HeaderValue::from_static("http://127.0.0.1:3000"),
        ]))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
        ]);

    // X-Request-Id propagation. If the caller (Electron / frontend
    // / Word add-in) supplies an X-Request-Id, we keep it; otherwise
    // we mint a fresh one. The same ID is reflected back on the
    // response so the client can correlate logs, and is available
    // via the request extensions for sidecar fan-out.
    //
    // Anti-pattern alignment: docs/05-edges.md says request IDs
    // propagate to sidecar calls. The supervisor's typed client
    // (Phase 3) will read this from the extensions when making
    // /parse calls, so logs can be `grep`'d across processes.
    let request_id = SetRequestIdLayer::x_request_id(MikeRequestId);
    let propagate_request_id = PropagateRequestIdLayer::x_request_id();

    // Snapshot the paths before `with_state` consumes `state` so the
    // post-bind audit::log call below can still see the workspace.
    let paths_for_audit = state.paths.clone();

    let app: Router<()> = Router::new()
        .route("/health", get(health))
        .nest("/internal", routes::internal::router())
        .nest("/system",   routes::system::router())
        .nest("/user",     routes::user::router())
        .nest("/chat",     routes::chat::router())
        .nest("/project",  routes::projects::router())
        .nest("/projects", routes::projects::router())
        .nest("/client",   routes::clients::router())
        .nest("/matter",   routes::matters::router())
        .nest("/document", routes::documents::router())
        // Alias used by the upstream-Mike frontend for standalone documents.
        .nest("/single-documents", routes::documents::router())
        .nest("/workflow",  routes::workflows::router())
        .nest("/workflows", routes::workflows::router())
        .nest("/tabular-review", routes::tabular_reviews::router())
        .nest("/sync",     routes::sync::router())
        .layer(propagate_request_id)
        .layer(request_id)
        .layer(middleware::from_fn(validate_host))
        .layer(cors)
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound = listener.local_addr()?;
    write_backend_runtime(&bound)?;
    audit::log(&paths_for_audit, audit::AuditEvent::WorkspaceOpened);
    println!("READY");
    tracing::info!("API listening on {bound}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(_auth: auth::middleware::AuthUser) -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

/// Mints fresh X-Request-Id values when callers don't supply one.
/// Uses a UUIDv4 because we don't (yet) depend on `ulid` in this
/// crate; the request_id is opaque to consumers so the format is
/// only a debugging convention. Swap to ULID if/when we want
/// sortable IDs for the audit log.
#[derive(Clone, Default)]
struct MikeRequestId;

impl MakeRequestId for MikeRequestId {
    fn make_request_id<B>(&mut self, _req: &axum::http::Request<B>) -> Option<RequestId> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        axum::http::HeaderValue::from_str(&id)
            .ok()
            .map(RequestId::new)
    }
}

async fn validate_host(req: Request, next: Next) -> Result<Response, StatusCode> {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let hostname = if host.starts_with('[') {
        host.split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or_else(|| host.to_string())
    } else {
        host.split(':').next().unwrap_or(host).to_string()
    };
    let allowed = matches!(hostname.as_str(), "127.0.0.1" | "localhost" | "[::1]");
    if !allowed {
        return Ok((StatusCode::MISDIRECTED_REQUEST, "invalid host").into_response());
    }
    Ok(next.run(req).await)
}

fn write_backend_runtime(addr: &std::net::SocketAddr) -> anyhow::Result<()> {
    let paths = workspace::WorkspacePaths::from_env()?;
    let payload = json!({
        "port": addr.port(),
        "pid": std::process::id(),
        "started_at": chrono::Utc::now().to_rfc3339(),
        "bind": addr.to_string(),
    });
    workspace::write_json_atomic(&paths.runtime_backend_json(), &payload)
}
