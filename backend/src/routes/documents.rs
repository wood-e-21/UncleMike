use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use crate::{auth::middleware::AuthUser, storage::make_storage, AppState};

fn storage_root() -> PathBuf {
    std::env::var("STORAGE_PATH")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("WORKSPACE_PATH").map(|w| PathBuf::from(w).join(".mike").join("storage")))
        .unwrap_or_else(|_| PathBuf::from("./data/storage"))
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

fn extract_text_for_upload(path: &FsPath, data: &[u8]) -> anyhow::Result<(String, Option<String>)> {
    #[cfg(feature = "rag")]
    {
        return crate::sync::scanner::extract_text_dispatch(path, data);
    }

    #[cfg(not(feature = "rag"))]
    {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "txt" | "md" | "csv") {
            Ok((String::from_utf8_lossy(data).into_owned(), None))
        } else {
            Ok((
                String::new(),
                Some("text extraction unavailable in this build".to_string()),
            ))
        }
    }
}

pub fn router() -> Router<Arc<AppState>> {
    // axum's DefaultBodyLimit caps multipart bodies at 2 MB out of the box.
    // A handful of docx/pdf docs uploaded together blow past that and the
    // connection is reset mid-stream — the browser surfaces it as
    // `TypeError: Failed to fetch`, not as an HTTP 413, which is why the
    // backend log shows nothing when concurrent uploads fail. 100 MB is
    // safely above any realistic legal document we expect.
    Router::new()
        .route("/", get(list_documents).post(upload_document))
        .route("/{id}", get(get_document).patch(update_document).delete(delete_document))
        // Display endpoint used by the in-app viewer (DocView.tsx / DocxView.tsx).
        // Returns the file bytes with the appropriate Content-Type so the
        // frontend can pick PDF.js or docx-preview based on it.
        .route("/{id}/display", get(display_document))
        .route("/{id}/docx", get(display_document))
        .route("/{id}/text", get(display_document))
        .route("/{id}/url", get(get_document_url))
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
}

// ---------------------------------------------------------------------------
// GET /document?project_id=…
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct ListQuery {
    project_id: Option<String>,
}

#[derive(Deserialize)]
struct UpdateDocumentBody {
    project_id: Option<String>,
    folder_id: Option<String>,
}

async fn list_documents(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult {
    let rows: Vec<(String, String, String, i64, Option<String>, String, Option<String>)> = if let Some(pid) = &q.project_id {
        sqlx::query_as(
            "SELECT id, filename, file_type, size_bytes, status, created_at, folder_id \
             FROM documents WHERE user_id = ? AND project_id = ? ORDER BY created_at DESC",
        )
        .bind(&auth.user_id)
        .bind(pid)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as(
            "SELECT id, filename, file_type, size_bytes, status, created_at, folder_id \
             FROM documents WHERE user_id = ? ORDER BY created_at DESC",
        )
        .bind(&auth.user_id)
        .fetch_all(&state.db)
        .await
    }
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let docs: Vec<Value> = rows
        .into_iter()
        .map(|(id, filename, file_type, size, status, created_at, folder_id)| {
            json!({ "id": id, "filename": filename, "file_type": file_type,
                    "size_bytes": size, "status": status, "created_at": created_at,
                    "folder_id": folder_id })
        })
        .collect();

    Ok(Json(json!({ "documents": docs })))
}

// ---------------------------------------------------------------------------
// POST /document  — multipart upload
// Fields: file (binary), project_id? (text)
// ---------------------------------------------------------------------------
async fn upload_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> ApiResult {
    tracing::info!("[upload] POST /document user={}", auth.user_id);
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut project_id: Option<String> = None;
    // `cache=true` is the chat-composer signal: store the binary +
    // extracted text under data/storage/cache, keyed by SHA-256 of the
    // bytes. The chat row may not exist at upload time (the composer
    // materialises the chat on first send), so chat_id is wired up
    // later by the /chat send handler — and the chat-delete handler
    // ref-counts by content_hash before unlinking the on-disk files.
    let mut cache = false;
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        tracing::warn!("[upload] multipart parse error: {e}");
        err(StatusCode::BAD_REQUEST, &e.to_string())
    })? {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                let bytes = field.bytes().await.map_err(|e| {
                    tracing::warn!(
                        "[upload] failed reading file field (filename={:?}): {e}",
                        filename
                    );
                    err(StatusCode::BAD_REQUEST, &e.to_string())
                })?;
                tracing::info!(
                    "[upload] received file field name={:?} size={} bytes",
                    filename,
                    bytes.len()
                );
                file_bytes = Some(bytes.to_vec());
            }
            "project_id" => {
                let text = field.text().await.map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
                if !text.trim().is_empty() {
                    project_id = Some(text.trim().to_string());
                }
            }
            "cache" => {
                let text = field.text().await.map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
                cache = matches!(text.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes");
            }
            _ => {}
        }
    }

    let data = file_bytes.ok_or_else(|| err(StatusCode::BAD_REQUEST, "No file field in multipart"))?;
    let fname = filename.unwrap_or_else(|| "upload".to_string());
    let ext = fname.rsplit('.').next().unwrap_or("").to_lowercase();
    let file_type = match ext.as_str() {
        "pdf" => "pdf",
        "docx" => "docx",
        "rtf" => "rtf",
        "xlsx" => "xlsx",
        "xls" => "xls",
        "xlsb" => "xlsb",
        "ods" => "ods",
        "csv" => "csv",
        "txt" => "txt",
        "md" => "md",
        "png" => "png",
        "jpg" | "jpeg" => "jpeg",
        "tif" | "tiff" => "tiff",
        _ => "other",
    };

    let doc_id = uuid::Uuid::new_v4().to_string();
    let storage = make_storage().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let size = data.len() as i64;

    // Cache uploads (chat-attached): key files by SHA-256 of the
    // binary so re-uploads of identical content dedupe and same
    // user-facing filename across different chats can't collide on
    // disk. We also extract plain text once per unique hash so the
    // chat send handler doesn't re-parse a 200-page PDF on every
    // turn. Skip extraction silently if the binary or text already
    // exist on disk — same hash means identical bytes.
    let mut matter_id = project_id.clone();
    let mut client_id: Option<String> = None;
    let mut item_path: Option<String> = None;

    let (storage_key, content_hash, extracted_text_path) = if cache {
        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(&data);
            format!("{:x}", hasher.finalize())
        };
        let bin_ext = if ext.is_empty() { "bin".to_string() } else { ext.clone() };
        let bin_key = format!("cache/{}.{}", hash, bin_ext);
        let txt_key = format!("cache/{}.txt", hash);

        let root = storage_root();
        let bin_abs = root.join(bin_key.replace('/', std::path::MAIN_SEPARATOR_STR));
        let txt_abs = root.join(txt_key.replace('/', std::path::MAIN_SEPARATOR_STR));

        if !bin_abs.exists() {
            storage
                .put(&bin_key, &data, "application/octet-stream")
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
            tracing::info!("[upload] cache binary written: {} ({} bytes)", bin_key, data.len());
        } else {
            tracing::info!("[upload] cache binary already exists, reusing: {}", bin_key);
        }

        if !txt_abs.exists() {
            // extract_text_dispatch keys off the path's extension, so
            // the absolute path of the binary we just wrote is the
            // right thing to feed it (pdfium also needs an on-disk
            // path for PDFs).
            match extract_text_for_upload(&bin_abs, &data) {
                Ok((text, skip_reason)) => {
                    if let Some(reason) = skip_reason {
                        tracing::info!(
                            "[upload] cache text extraction skipped for {} ({}): {}",
                            fname,
                            hash,
                            reason
                        );
                    }
                    storage
                        .put(&txt_key, text.as_bytes(), "text/plain; charset=utf-8")
                        .await
                        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
                    tracing::info!(
                        "[upload] cache text written: {} ({} chars)",
                        txt_key,
                        text.len()
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "[upload] cache text extraction failed for {} ({}): {}",
                        fname,
                        hash,
                        e
                    );
                    // Drop a marker so we don't retry on every reload —
                    // an empty .txt is a valid "we tried" signal.
                    let _ = storage
                        .put(&txt_key, b"", "text/plain; charset=utf-8")
                        .await;
                }
            }
        } else {
            tracing::info!("[upload] cache text already exists, reusing: {}", txt_key);
        }

        (bin_key, Some(hash), Some(txt_key))
    } else {
        // Legacy (non-cache) layout: per-user, per-doc-id. No hashing,
        // no text extraction — the existing pipeline handles those
        // documents on demand.
        let key = format!("documents/{}/{}", auth.user_id, doc_id);
        storage
            .put(&key, &data, "application/octet-stream")
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        let (resolved_matter_id, matter_slug, resolved_client_id) = if let Some(pid) = project_id.as_deref() {
            let (slug, cid) = crate::routes::matters::matter_slug(&state, &auth.user_id, pid).await?;
            (pid.to_string(), slug, cid)
        } else {
            crate::routes::matters::ensure_default_matter(&state, &auth.user_id).await?
        };
        matter_id = Some(resolved_matter_id.clone());
        client_id = Some(resolved_client_id.clone());

        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(&data);
            format!("{:x}", hasher.finalize())
        };
        let attachment_ext = if ext.is_empty() { "bin".to_string() } else { ext.clone() };
        let attachment_name = format!("{}.{}", &hash[..12], attachment_ext);
        let matter_dir = if matter_slug == "_unfiled" {
            state.paths.unfiled_matter_dir()
        } else {
            let client_slug: String = sqlx::query_scalar("SELECT slug FROM clients WHERE id = ?")
                .bind(&resolved_client_id)
                .fetch_one(&state.db)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
            state.paths.matters_dir.join(client_slug).join(&matter_slug)
        };
        let attachment_path = matter_dir.join("attachments").join(&attachment_name);
        crate::workspace::write_atomic(&attachment_path, &data)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        let (body_text, _) = extract_text_for_upload(&attachment_path, &data)
            .unwrap_or_else(|_| ("".to_string(), Some("text extraction failed".to_string())));
        let body_hash = {
            let mut hasher = Sha256::new();
            hasher.update(body_text.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        let fm = json!({
            "id": doc_id,
            "schema_version": 1,
            "kind": "document",
            "matter_id": resolved_matter_id,
            "client_id": resolved_client_id,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "updated_at": chrono::Utc::now().to_rfc3339(),
            "content_hash": format!("sha256:{body_hash}"),
            "title": fname.clone(),
            "tags": [],
            "isolation_mode": "shared",
            "attachments": [{
                "sha256": hash.clone(),
                "filename": fname.clone(),
                "mime": "application/octet-stream",
                "size_bytes": size
            }],
            "source": {
                "kind": "upload",
                "original_filename": fname.clone(),
                "uploaded_at": chrono::Utc::now().to_rfc3339(),
                "parser": "rust"
            },
            "custom": {}
        });
        let yaml = serde_yaml::to_string(&fm)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        let item_abs = matter_dir.join("items").join(format!("document-{doc_id}.md"));
        crate::workspace::write_atomic(
            &item_abs,
            format!("---\n{yaml}---\n\n{body_text}\n").as_bytes(),
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        item_path = Some(
            item_abs
                .strip_prefix(&state.paths.root)
                .unwrap_or(&item_abs)
                .to_string_lossy()
                .to_string(),
        );

        let key = attachment_path
            .strip_prefix(&state.paths.root)
            .unwrap_or(&attachment_path)
            .to_string_lossy()
            .to_string();
        (key, Some(hash), None)
    };

    sqlx::query(
        "INSERT INTO documents (id, user_id, project_id, matter_id, client_id, filename, file_type, size_bytes, storage_path, item_path, status, content_hash, extracted_text_path) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'ready', ?, ?)",
    )
    .bind(&doc_id)
    .bind(&auth.user_id)
    .bind(&project_id)
    .bind(&matter_id)
    .bind(&client_id)
    .bind(&fname)
    .bind(file_type)
    .bind(size)
    .bind(&storage_key)
    .bind(&item_path)
    .bind(&content_hash)
    .bind(&extracted_text_path)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({
        "id": doc_id,
        "filename": fname,
        "file_type": file_type,
        "size_bytes": size,
        "status": "ready"
    })))
}

// ---------------------------------------------------------------------------
// GET /document/:id
// ---------------------------------------------------------------------------
async fn get_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(String, String, String, i64, Option<String>, Option<String>, String, Option<String>)> =
        sqlx::query_as(
            "SELECT id, filename, file_type, size_bytes, storage_path, status, created_at, folder_id \
             FROM documents WHERE id = ? AND user_id = ?",
        )
        .bind(&id)
        .bind(&auth.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (id, filename, file_type, size, storage_path, status, created_at, folder_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Document not found"))?;

    Ok(Json(json!({
        "id": id,
        "filename": filename,
        "file_type": file_type,
        "size_bytes": size,
        "storage_path": storage_path,
        "status": status,
        "created_at": created_at,
        "folder_id": folder_id,
    })))
}

async fn update_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateDocumentBody>,
) -> ApiResult {
    if let Some(project_id) = body.project_id.as_deref() {
        let (_matter_slug, client_id) =
            crate::routes::matters::matter_slug(&state, &auth.user_id, project_id).await?;
        let result = sqlx::query(
            "UPDATE documents SET project_id = ?, matter_id = ?, client_id = ? \
             WHERE id = ? AND user_id = ?",
        )
        .bind(project_id)
        .bind(project_id)
        .bind(client_id)
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        if result.rows_affected() == 0 {
            return Err(err(StatusCode::NOT_FOUND, "Document not found"));
        }
    }

    if let Some(folder_id) = body.folder_id.as_deref() {
        let result = sqlx::query(
            "UPDATE documents SET folder_id = ? WHERE id = ? AND user_id = ?",
        )
        .bind(folder_id)
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        if result.rows_affected() == 0 {
            return Err(err(StatusCode::NOT_FOUND, "Document not found"));
        }
    }

    get_document(State(state), auth, Path(id)).await
}

// ---------------------------------------------------------------------------
// GET /document/:id/display, /docx, /text — stream raw bytes for the viewer
// ---------------------------------------------------------------------------
async fn display_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Response {
    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT filename, file_type, storage_path FROM documents WHERE id = ? AND user_id = ?",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some((filename, file_type, Some(storage_path))) = row else {
        return (StatusCode::NOT_FOUND, "Document not found").into_response();
    };

    let bytes = if storage_path.starts_with("matters/") {
        match tokio::fs::read(state.paths.root.join(&storage_path)).await {
            Ok(b) => b,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    } else {
        let storage = match crate::storage::make_storage() {
            Ok(s) => s,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        match storage.get(&storage_path).await {
            Ok(b) => b,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    };

    let content_type = match file_type.as_str() {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "rtf" => "application/rtf",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "csv" => "text/csv; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "png" => "image/png",
        "jpeg" | "jpg" => "image/jpeg",
        "tiff" | "tif" => "image/tiff",
        _ => "application/octet-stream",
    };

    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(disp) = HeaderValue::from_str(&format!("inline; filename=\"{filename}\"")) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, disp);
    }
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=60"),
    );
    resp
}

// ---------------------------------------------------------------------------
// GET /document/:id/url — frontend convenience: returns a URL the viewer
// can fetch later. In MikeRust it's just an absolute /display URL because
// storage is local; remote-storage backends could return a presigned URL.
// ---------------------------------------------------------------------------
async fn get_document_url(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let owns: Option<(String,)> =
        sqlx::query_as("SELECT id FROM documents WHERE id = ? AND user_id = ?")
            .bind(&id)
            .bind(&auth.user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if owns.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Document not found"));
    }
    let api_base = std::env::var("API_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:3001".to_string());
    Ok(Json(json!({
        "url": format!("{api_base}/document/{id}/display"),
    })))
}

// ---------------------------------------------------------------------------
// DELETE /document/:id
// ---------------------------------------------------------------------------
async fn delete_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT storage_path FROM documents WHERE id = ? AND user_id = ?")
            .bind(&id)
            .bind(&auth.user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (storage_path,) = row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Document not found"))?;

    // Delete from storage
    if let Some(key) = storage_path {
        if let Ok(storage) = make_storage() {
            let _ = storage.delete(&key).await;
        }
    }

    sqlx::query("DELETE FROM documents WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "ok": true })))
}
