use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::middleware::AuthUser, workspace, AppState};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "detail": msg })))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_clients).post(create_client))
        .route("/{id}", get(get_client).patch(update_client).put(update_client).delete(delete_client))
}

#[derive(Deserialize)]
struct ClientBody {
    name: String,
    notes: Option<String>,
}

async fn list_clients(State(state): State<Arc<AppState>>, auth: AuthUser) -> ApiResult {
    let rows: Vec<(String, String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, name, slug, notes, created_at, updated_at \
         FROM clients WHERE user_id = ? ORDER BY updated_at DESC",
    )
    .bind(&auth.user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({
        "clients": rows.into_iter().map(|(id, name, slug, notes, created_at, updated_at)| {
            json!({ "id": id, "name": name, "slug": slug, "notes": notes,
                    "created_at": created_at, "updated_at": updated_at })
        }).collect::<Vec<_>>()
    })))
}

async fn create_client(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<ClientBody>,
) -> ApiResult {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Client name cannot be empty"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let slug = workspace::slugify(name);
    sqlx::query(
        "INSERT INTO clients (id, user_id, name, slug, notes) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .bind(name)
    .bind(&slug)
    .bind(&body.notes)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let _ = write_client_md(&state, &id).await?;
    Ok(Json(json!({ "id": id, "name": name, "slug": slug })))
}

async fn get_client(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(String, String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, name, slug, notes, created_at, updated_at \
         FROM clients WHERE id = ? AND user_id = ?",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (id, name, slug, notes, created_at, updated_at) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Client not found"))?;
    Ok(Json(json!({ "id": id, "name": name, "slug": slug, "notes": notes,
                    "created_at": created_at, "updated_at": updated_at })))
}

async fn update_client(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ClientBody>,
) -> ApiResult {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Client name cannot be empty"));
    }
    let result = sqlx::query(
        "UPDATE clients SET name = ?, notes = ?, updated_at = datetime('now') \
         WHERE id = ? AND user_id = ?",
    )
    .bind(name)
    .bind(&body.notes)
    .bind(&id)
    .bind(&auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Client not found"));
    }
    let _ = write_client_md(&state, &id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_client(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let result = sqlx::query("DELETE FROM clients WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Client not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn write_client_md(state: &AppState, id: &str) -> ApiResult {
    let row: (String, String, String, Option<String>, String, String) = sqlx::query_as(
        "SELECT id, name, slug, notes, created_at, updated_at FROM clients WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (id, name, slug, notes, created_at, updated_at) = row;
    let dir = state.paths.matters_dir.join(&slug);
    std::fs::create_dir_all(&dir).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let frontmatter = json!({
        "id": id,
        "schema_version": 1,
        "kind": "client",
        "name": name,
        "slug": slug,
        "created_at": created_at,
        "updated_at": updated_at,
    });
    let yaml = serde_yaml::to_string(&frontmatter)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let body = notes.unwrap_or_default();
    workspace::write_atomic(&dir.join("client.md"), format!("---\n{yaml}---\n\n{body}\n").as_bytes())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}
