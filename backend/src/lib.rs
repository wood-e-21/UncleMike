pub mod auth;
pub mod db;
pub mod embeddings;
pub mod llm;
pub mod mcp;
pub mod mikeprj;
pub mod pdf;
pub mod routes;
pub mod storage;
pub mod sync;
pub mod workspace;

pub use db::AppState;

use axum::{
    extract::Request,
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

pub async fn run_server(port: u16) -> anyhow::Result<()> {
    run_server_inner(port).await
}

/// Load `.env` from a known-good location regardless of cwd.
///
/// Electron can spawn the bundled backend from a resources directory where
/// there may be no `.env`. Plain `dotenvy::dotenv()` only checks cwd, so we
/// walk up from both cwd and the executable directory until we find one.
fn load_dotenv() {
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

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers(Any);

    let app: Router<()> = Router::new()
        .route("/health", get(health))
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
        .nest("/tabular-review", routes::tabular_reviews::router())
        .nest("/sync",     routes::sync::router())
        .layer(middleware::from_fn(validate_host))
        .layer(cors)
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound = listener.local_addr()?;
    write_backend_runtime(&bound)?;
    println!("READY");
    tracing::info!("API listening on {bound}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(_auth: auth::middleware::AuthUser) -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
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
