PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS user_profiles (
    id              TEXT PRIMARY KEY,
    username        TEXT UNIQUE NOT NULL,
    email           TEXT,
    display_name    TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS sessions (
    token       TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS clients (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    name        TEXT NOT NULL,
    slug        TEXT NOT NULL,
    notes       TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, slug)
);

CREATE TABLE IF NOT EXISTS matters (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    client_id       TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT,
    slug            TEXT NOT NULL,
    isolation_mode  TEXT NOT NULL DEFAULT 'shared' CHECK (isolation_mode IN ('shared', 'strict')),
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, client_id, slug)
);

-- TODO(phase-cleanup): the corpus_* + fetched_with_fallback columns are
-- vestigial from MikeRust's EUR-Lex / Italian-legal corpora. The Phase 1
-- reshape stripped the corpus *features* but left these columns because
-- routes/chat.rs and routes/sync.rs still reference them. They should be
-- dropped once those routes are cleaned up — currently tracked as a
-- separate cleanup PR.
CREATE TABLE IF NOT EXISTS documents (
    id                   TEXT PRIMARY KEY,
    user_id              TEXT NOT NULL,
    project_id           TEXT REFERENCES matters(id) ON DELETE SET NULL,
    matter_id            TEXT REFERENCES matters(id) ON DELETE SET NULL,
    client_id            TEXT REFERENCES clients(id) ON DELETE SET NULL,
    folder_id            TEXT,
    filename             TEXT NOT NULL,
    file_type            TEXT NOT NULL,
    size_bytes           INTEGER NOT NULL DEFAULT 0,
    storage_path         TEXT,
    item_path            TEXT,
    content_hash         TEXT,
    extracted_text_path  TEXT,
    corpus_id            TEXT,    -- TODO drop with corpus cleanup
    corpus_identifier    TEXT,    -- TODO drop with corpus cleanup
    corpus_language      TEXT,    -- TODO drop with corpus cleanup
    fetched_with_fallback INTEGER NOT NULL DEFAULT 0,  -- TODO drop with corpus cleanup
    chat_id              TEXT,
    status               TEXT NOT NULL DEFAULT 'ready',
    created_at           TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS document_versions (
    id              TEXT PRIMARY KEY,
    document_id     TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    version_number  INTEGER NOT NULL,
    storage_path    TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS chats (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    project_id      TEXT REFERENCES matters(id) ON DELETE CASCADE,
    matter_id       TEXT REFERENCES matters(id) ON DELETE CASCADE,
    title           TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS messages (
    id              TEXT PRIMARY KEY,
    chat_id         TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK (role IN ('user','assistant','tool','system')),
    content         TEXT,
    files           TEXT,
    workflow        TEXT,
    annotations     TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS workflows (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    title           TEXT NOT NULL,
    prompt_md       TEXT,
    type            TEXT NOT NULL DEFAULT 'assistant',
    practice        TEXT,
    columns_config  TEXT NOT NULL DEFAULT '[]',
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS workflow_hidden (
    user_id      TEXT NOT NULL,
    workflow_id  TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(user_id, workflow_id)
);

CREATE TABLE IF NOT EXISTS tabular_reviews (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    project_id      TEXT REFERENCES matters(id) ON DELETE SET NULL,
    matter_id       TEXT REFERENCES matters(id) ON DELETE SET NULL,
    workflow_id     TEXT REFERENCES workflows(id) ON DELETE SET NULL,
    title           TEXT,
    columns_config  TEXT NOT NULL DEFAULT '[]',
    status          TEXT NOT NULL DEFAULT 'pending',
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS tabular_review_rows (
    id                 TEXT PRIMARY KEY,
    tabular_review_id  TEXT NOT NULL REFERENCES tabular_reviews(id) ON DELETE CASCADE,
    document_id        TEXT REFERENCES documents(id) ON DELETE SET NULL,
    row_index          INTEGER NOT NULL DEFAULT 0,
    cells              TEXT NOT NULL DEFAULT '[]',
    status             TEXT NOT NULL DEFAULT 'pending',
    created_at         TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS user_settings (
    user_id          TEXT PRIMARY KEY,
    main_model       TEXT,
    title_model      TEXT,
    tabular_model    TEXT,
    claude_api_key   TEXT,
    gemini_api_key   TEXT,
    gemini_region    TEXT,
    gemini_model     TEXT,
    openai_api_key   TEXT,
    openai_model     TEXT,
    local_base_url   TEXT,
    local_api_key    TEXT,
    local_model      TEXT,
    active_provider  TEXT,
    locale           TEXT,
    mcp_servers      TEXT NOT NULL DEFAULT '[]',
    updated_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS sync_folders (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    project_id  TEXT REFERENCES matters(id) ON DELETE SET NULL,
    matter_id   TEXT REFERENCES matters(id) ON DELETE SET NULL,
    path        TEXT NOT NULL,
    label       TEXT,
    recursive   INTEGER NOT NULL DEFAULT 1,
    status      TEXT NOT NULL DEFAULT 'idle',
    last_scan_at TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS synced_files (
    id            TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL,
    folder_id     TEXT REFERENCES sync_folders(id) ON DELETE CASCADE,
    project_id    TEXT REFERENCES matters(id) ON DELETE SET NULL,
    matter_id     TEXT REFERENCES matters(id) ON DELETE SET NULL,
    path          TEXT NOT NULL,
    filename      TEXT NOT NULL,
    file_type     TEXT NOT NULL,
    document_id   TEXT,
    sha256        TEXT,
    mtime         TEXT,
    size_bytes    INTEGER NOT NULL DEFAULT 0,
    content_hash  TEXT,
    status        TEXT NOT NULL DEFAULT 'pending',
    skip_reason   TEXT,
    chunk_count   INTEGER NOT NULL DEFAULT 0,
    scanned_at    TEXT,
    indexed_at    TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS mcp_servers (
    user_id     TEXT NOT NULL,
    name        TEXT NOT NULL,
    transport   TEXT NOT NULL DEFAULT 'http',
    url         TEXT,
    command     TEXT,
    args_json   TEXT NOT NULL DEFAULT '[]',
    env_json    TEXT NOT NULL DEFAULT '{}',
    headers_json TEXT NOT NULL DEFAULT '{}',
    api_key     TEXT,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(user_id, name)
);

CREATE INDEX IF NOT EXISTS idx_documents_user ON documents(user_id);
CREATE INDEX IF NOT EXISTS idx_documents_matter ON documents(matter_id);
CREATE INDEX IF NOT EXISTS idx_chats_user ON chats(user_id);
CREATE INDEX IF NOT EXISTS idx_chats_matter ON chats(matter_id);
CREATE INDEX IF NOT EXISTS idx_matters_client ON matters(client_id);
