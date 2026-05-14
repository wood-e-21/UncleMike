use anyhow::Result;
use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::{
    auth::SessionStore,
    secrets::SharedSecrets,
    sidecars::supervisor::Supervisor,
    workspace::WorkspacePaths,
};

pub mod cipher;
pub mod models;
pub mod repositories;

#[cfg(feature = "rag")]
use crate::embeddings::EmbeddingService;
#[cfg(feature = "rag")]
use crate::sync::scanner::ScanProgressHandle;

/// Legacy biometric request channel retained only for older helper code that
/// still compiles against AppState; Electron now owns unlock/PIN handling.
pub type BiometricRequest = (String, oneshot::Sender<Result<bool, String>>);

/// How long an MCP discovery snapshot stays valid before we re-run the
/// `initialize → tools/list → prompts/list` handshake. Five minutes
/// matches the typical horizon at which an MCP server might cycle a
/// session id; before this cache, every chat turn paid the full
/// handshake cost on every configured server. Configurable via env
/// override `MCP_CACHE_TTL_SECS` for tuning / tests.
pub fn mcp_cache_ttl() -> std::time::Duration {
    std::env::var("MCP_CACHE_TTL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or_else(|| std::time::Duration::from_secs(300))
}

/// How long to wait for an MCP `tools/call` response before giving up.
/// Default 5 minutes — pseudonymization, OCR, RAG-summary tools can
/// realistically take 60-120 s on a non-trivial doc, and the previous
/// 60 s default tripped over them: every long call returned an opaque
/// `{"error":"network: timeout"}` string and the model would tell the
/// user "communication error" instead of waiting. Override via env
/// `MCP_CALL_TIMEOUT_SECS` for shops that prefer to fail faster.
pub fn mcp_call_timeout_secs() -> u64 {
    std::env::var("MCP_CALL_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0 && n <= 1800)
        .unwrap_or(300)
}

/// Stored per-user. We carry the discovery payload as opaque JSON so
/// `db/mod.rs` doesn't depend on the routes layer's `McpDiscovered`
/// type — the chat handler serialises into this on insert and
/// deserialises on read. Cheap (a handful of MCP servers per user;
/// each one a few hundred bytes of JSON).
#[derive(Clone)]
pub struct McpDiscoveryCacheEntry {
    pub stored_at: std::time::Instant,
    /// JSON-encoded `Vec<McpDiscovered>`. Kept as a string to keep
    /// this module dependency-free.
    pub payload_json: String,
}

impl McpDiscoveryCacheEntry {
    pub fn is_fresh(&self, ttl: std::time::Duration) -> bool {
        self.stored_at.elapsed() < ttl
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub paths: WorkspacePaths,
    pub sessions: SessionStore,
    /// Legacy biometric channel. Electron Phase 0+1 does not route unlocks
    /// through the backend.
    pub biometric_tx: Option<mpsc::Sender<BiometricRequest>>,
    /// Cache of model identifiers that the upstream LLM provider has rejected
    /// for `tools=[…]` (e.g. Ollama Gemma3 returns "does not support tools").
    /// Avoids paying the round-trip on every chat request.
    pub no_tools_models: Arc<RwLock<HashSet<String>>>,

    /// Per-user MCP discovery cache. Avoids re-running the
    /// `initialize → notifications/initialized → tools/list → prompts/list`
    /// handshake on every chat turn — without this every user message
    /// hammered every configured MCP server with a fresh session id.
    /// TTL-based; entries older than `MCP_CACHE_TTL` are re-discovered
    /// on the next chat. Manually invalidated when the user updates
    /// MCP server settings (POST/PUT/DELETE on /user/mcp).
    pub mcp_discovery_cache:
        Arc<RwLock<HashMap<String, McpDiscoveryCacheEntry>>>,

    /// Process-wide embedding service (loads multilingual-e5-base once
    /// on first use and reuses it). `None` when the `rag` feature is
    /// disabled at compile time, OR when STORAGE_PATH wasn't configured
    /// (which we need as the on-disk root for per-user Lance dbs).
    #[cfg(feature = "rag")]
    pub embeddings: Option<Arc<EmbeddingService>>,

    /// In-memory map of in-flight scan progress, keyed by `sync_folders.id`.
    /// Populated when `/sync/folders/{id}/scan` kicks off a job; read by
    /// the status endpoint. Cleared when the user removes the folder.
    #[cfg(feature = "rag")]
    pub scans: Arc<RwLock<HashMap<String, ScanProgressHandle>>>,

    /// In-process secrets bundle (API keys for upstream LLMs, etc.).
    /// Empty until Electron POSTs to `/internal/secrets/load` after the
    /// backend's `READY` token. Never written to disk by the backend;
    /// the on-disk copy lives in `<workspace>/.mike/secrets.enc` and is
    /// owned by Electron. See `backend/src/secrets/mod.rs` for the
    /// access pattern and `docs/decisions.md` for the architectural
    /// decision.
    pub secrets: SharedSecrets,

    /// Sidecar supervisor — the Rust-side membrane in front of Python
    /// sidecars (Docling today; eyecite, presidio later). At Phase 1
    /// the supervisor is read-only (it probes Electron-spawned
    /// processes); at Phase 3 it will own spawn + restart-with-backoff.
    /// See `backend/src/sidecars/`.
    pub sidecars: Arc<Supervisor>,
}

impl AppState {
    pub async fn new() -> Result<Self> {
        // Register sqlite-vec as a SQLite auto-extension BEFORE we open
        // any connection. This way every connection sqlx creates
        // (including the one running migrations) gets the `vec0`
        // virtual-table module loaded — required by migration 0009 and
        // by every embedding query later on.
        //
        // The cast goes through `*const ()` because libsqlite3-sys'
        // `sqlite3_auto_extension` expects a generic init function
        // pointer, while sqlite-vec exposes a specifically-typed one.
        // Both ABIs match what SQLite calls at extension load time.
        #[cfg(feature = "rag")]
        {
            crate::embeddings::register_sqlite_vec_auto_extension();
            tracing::info!("[rag] sqlite-vec auto-extension registered");
        }

        let paths = WorkspacePaths::from_env()?;
        let db_url = paths.db_url();

        // SQLite won't auto-create the parent directory; do it explicitly
        // so `<workspace>/.mike/` exists on first run.
        if let Some(file_path) = db_url.strip_prefix("sqlite:") {
            // Strip query string if any (e.g. ?mode=rwc) before mkdir.
            let raw = file_path.split('?').next().unwrap_or(file_path);
            // Tolerate both `/` and `\` in the URL.
            let pb = PathBuf::from(raw.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = pb.parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
        }
        tracing::info!("[db] using workspace db={}", paths.db_path.display());

        // SQLCipher key. Derived from MIKE_BACKEND_UNLOCK_SECRET (set by
        // Electron at spawn time). PRAGMA must run BEFORE any other
        // statement on a connection, so we install it as a pragma in
        // the connect options. sqlx pipes pragmas via the connect
        // options' `pragma(name, value)` API; that runs as part of
        // connection setup, before our statements see the DB.
        let cipher_key = cipher::database_key_hex()?;
        let opts = SqliteConnectOptions::from_str(&db_url)?
            .create_if_missing(true)
            .pragma("key", format!("\"x'{cipher_key}'\""))
            .pragma("cipher_compatibility", "4")
            .pragma("cipher_memory_security", "ON")
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            // Suppress the long-statement warning for `PRAGMA key` —
            // we know it's slow on first connection (SQLCipher's PBKDF
            // for the legacy raw-string form would matter; raw hex is
            // fast, but log noise on cold start isn't useful).
            .log_slow_statements(tracing::log::LevelFilter::Trace, std::time::Duration::from_secs(1));

        let db = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .map_err(|e| {
                if let Some(hint) = cipher::explain_open_failure(&e) {
                    anyhow::anyhow!("{e} — {hint}")
                } else {
                    anyhow::anyhow!(e)
                }
            })?;

        // Confirm SQLCipher is actually live. Without this assertion
        // the binary will happily run against a plain sqlite if the
        // libsqlite3-sys feature flags ever drift. A "data" file on
        // disk is alpha criterion #9; we make it visible at boot too.
        match sqlx::query_scalar::<_, String>("PRAGMA cipher_version")
            .fetch_optional(&db)
            .await
        {
            Ok(Some(v)) => tracing::info!("[db] sqlcipher active, version={v}"),
            Ok(None) | Err(_) => {
                anyhow::bail!(
                    "SQLCipher is not active. The binary is linked against plain SQLite. \
                     Check `libsqlite3-sys` features in backend/Cargo.toml — \
                     `bundled-sqlcipher-vendored-openssl` is required."
                );
            }
        }

        let sessions = SessionStore::new(db.clone());

        // Bootstrap the RAG embedding service. The vector store lives
        // in the same SQLite file via the sqlite-vec virtual table —
        // we just hand the pool to the service. The migration adds the
        // `doc_chunks` vec0 table; if that already ran we're ready.
        #[cfg(feature = "rag")]
        let embeddings: Option<Arc<EmbeddingService>> =
            Some(Arc::new(EmbeddingService::new(db.clone())));

        let sidecars = crate::sidecars::supervisor::build_default(paths.root.clone())?;

        Ok(Self {
            db,
            paths,
            sessions,
            biometric_tx: None,
            no_tools_models: Arc::new(RwLock::new(HashSet::new())),
            mcp_discovery_cache: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "rag")]
            embeddings,
            #[cfg(feature = "rag")]
            scans: Arc::new(RwLock::new(HashMap::new())),
            secrets: crate::secrets::new_shared(),
            sidecars,
        })
    }

    /// Invalidate any cached MCP discovery for this user. Called from
    /// the /user/mcp settings endpoints whenever the user adds, edits
    /// or removes a server, so the next chat re-runs discovery instead
    /// of using a stale (possibly broken) tool list.
    pub async fn invalidate_mcp_cache_for_user(&self, user_id: &str) {
        let mut g = self.mcp_discovery_cache.write().await;
        g.remove(user_id);
    }

    pub async fn run_migrations(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.db).await?;
        #[cfg(feature = "rag")]
        self.ensure_doc_chunks_table().await?;
        self.sessions.purge_expired().await?;
        Ok(())
    }

    #[cfg(feature = "rag")]
    async fn ensure_doc_chunks_table(&self) -> Result<()> {
        sqlx::query(
            "CREATE VIRTUAL TABLE IF NOT EXISTS doc_chunks USING vec0(
                user_id      text partition key,
                project_id   text partition key,
                embedding    float[768],
                +document_id text,
                +source_path text,
                +chunk_index integer,
                +text        text,
                +page        integer
            )",
        )
        .execute(&self.db)
        .await?;
        Ok(())
    }
}
