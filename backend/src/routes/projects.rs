//! Temporary `/project` compatibility routes.
//!
//! The frontend baseline still speaks in "projects" in several places. These
//! routes map that shape onto the new `matters` table while the UI is renamed
//! to clients/matters.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::middleware::AuthUser, routes::matters, workspace, AppState};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "detail": msg })))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_projects).post(create_project))
        .route("/{id}", get(get_project).patch(update_project).put(update_project).delete(delete_project))
        .route("/{id}/export", post(export_project_placeholder))
        .route("/import", post(import_project_placeholder))
}

#[derive(Deserialize)]
struct ProjectBody {
    name: Option<String>,
    description: Option<String>,
    cm_number: Option<String>,
    isolation_mode: Option<String>,
}

async fn list_projects(State(state): State<Arc<AppState>>, auth: AuthUser) -> ApiResult {
    let rows: Vec<(String, String, Option<String>, String, String, String, i64, i64, i64)> = sqlx::query_as(
        "SELECT m.id, m.name, m.description, m.isolation_mode, m.created_at, m.updated_at, \
                (SELECT COUNT(*) FROM documents d WHERE d.user_id = m.user_id AND d.project_id = m.id), \
                (SELECT COUNT(*) FROM chats c WHERE c.user_id = m.user_id AND c.project_id = m.id), \
                (SELECT COUNT(*) FROM tabular_reviews tr WHERE tr.user_id = m.user_id AND tr.project_id = m.id) \
         FROM matters m \
         WHERE m.user_id = ? ORDER BY m.updated_at DESC",
    )
    .bind(&auth.user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let projects: Vec<Value> = rows
        .into_iter()
        .map(|(id, name, description, isolation_mode, created_at, updated_at, document_count, chat_count, review_count)| {
            json!({
                "id": id,
                "name": name,
                "description": description,
                "isolation_mode": isolation_mode,
                "created_at": created_at,
                "updated_at": updated_at,
                "document_count": document_count,
                "chat_count": chat_count,
                "review_count": review_count
            })
        })
        .collect();
    Ok(Json(json!({ "projects": projects })))
}

async fn create_project(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<ProjectBody>,
) -> ApiResult {
    let name = body
        .name
        .as_deref()
        .or(body.cm_number.as_deref())
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Matter name cannot be empty"));
    }
    let (default_matter_id, _, client_id) = matters::ensure_default_matter(&state, &auth.user_id).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let slug = workspace::slugify(name);
    let isolation = body.isolation_mode.as_deref().unwrap_or("shared");
    sqlx::query(
        "INSERT INTO matters (id, user_id, client_id, name, description, slug, isolation_mode) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .bind(&client_id)
    .bind(name)
    .bind(&body.description)
    .bind(&slug)
    .bind(isolation)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if default_matter_id == id {
        tracing::debug!("created default matter through project alias");
    }
    Ok(Json(json!({ "id": id, "name": name, "description": body.description })))
}

async fn get_project(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(String, String, Option<String>, String, String, String)> = sqlx::query_as(
        "SELECT id, name, description, isolation_mode, created_at, updated_at \
         FROM matters WHERE id = ? AND user_id = ?",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (id, name, description, isolation_mode, created_at, updated_at) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Matter not found"))?;
    let docs: Vec<(String, String, String, i64, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT id, filename, file_type, size_bytes, status, created_at, folder_id \
         FROM documents WHERE user_id = ? AND project_id = ? ORDER BY created_at DESC",
    )
    .bind(&auth.user_id)
    .bind(&id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let documents: Vec<Value> = docs
        .into_iter()
        .map(|(id, filename, file_type, size_bytes, status, created_at, folder_id)| {
            json!({
                "id": id,
                "filename": filename,
                "file_type": file_type,
                "size_bytes": size_bytes,
                "status": status,
                "created_at": created_at,
                "folder_id": folder_id
            })
        })
        .collect();
    Ok(Json(json!({ "id": id, "name": name, "description": description,
                    "isolation_mode": isolation_mode,
                    "created_at": created_at, "updated_at": updated_at,
                    "documents": documents, "folders": [] })))
}

async fn update_project(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ProjectBody>,
) -> ApiResult {
    let result = sqlx::query(
        "UPDATE matters SET \
           name = COALESCE(?, name), \
           description = COALESCE(?, description), \
           isolation_mode = COALESCE(?, isolation_mode), \
           updated_at = datetime('now') \
         WHERE id = ? AND user_id = ?",
    )
    .bind(body.name.as_deref())
    .bind(body.description.as_deref())
    .bind(body.isolation_mode.as_deref())
    .bind(&id)
    .bind(&auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Matter not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn delete_project(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let result = sqlx::query("DELETE FROM matters WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Matter not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn export_project_placeholder() -> ApiResult {
    Err(err(
        StatusCode::NOT_IMPLEMENTED,
        ".mikeprj export is being re-homed to matter exports in this reshape",
    ))
}

async fn import_project_placeholder() -> ApiResult {
    Err(err(
        StatusCode::NOT_IMPLEMENTED,
        ".mikeprj import is being re-homed to matter imports in this reshape",
    ))
}
