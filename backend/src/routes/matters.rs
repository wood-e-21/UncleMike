use axum::{
    extract::{Path, Query, State},
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
        .route("/", get(list_matters).post(create_matter))
        .route("/{id}", get(get_matter).patch(update_matter).put(update_matter).delete(delete_matter))
}

#[derive(Deserialize)]
struct MatterQuery {
    client_id: Option<String>,
}

#[derive(Deserialize)]
struct MatterBody {
    name: String,
    description: Option<String>,
    client_id: Option<String>,
    client_name: Option<String>,
    isolation_mode: Option<String>,
}

async fn list_matters(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(q): Query<MatterQuery>,
) -> ApiResult {
    let rows: Vec<(String, String, Option<String>, String, String, String, String, String)> =
        if let Some(client_id) = q.client_id {
            sqlx::query_as(
                "SELECT m.id, m.name, m.description, m.slug, m.client_id, c.name, \
                        m.isolation_mode, m.updated_at \
                 FROM matters m JOIN clients c ON c.id = m.client_id \
                 WHERE m.user_id = ? AND m.client_id = ? ORDER BY m.updated_at DESC",
            )
            .bind(&auth.user_id)
            .bind(client_id)
            .fetch_all(&state.db)
            .await
        } else {
            sqlx::query_as(
                "SELECT m.id, m.name, m.description, m.slug, m.client_id, c.name, \
                        m.isolation_mode, m.updated_at \
                 FROM matters m JOIN clients c ON c.id = m.client_id \
                 WHERE m.user_id = ? ORDER BY m.updated_at DESC",
            )
            .bind(&auth.user_id)
            .fetch_all(&state.db)
            .await
        }
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({
        "matters": rows.into_iter().map(|(id, name, description, slug, client_id, client_name, isolation_mode, updated_at)| {
            json!({ "id": id, "name": name, "description": description, "slug": slug,
                    "client_id": client_id, "client_name": client_name,
                    "isolation_mode": isolation_mode, "updated_at": updated_at })
        }).collect::<Vec<_>>()
    })))
}

async fn create_matter(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<MatterBody>,
) -> ApiResult {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Matter name cannot be empty"));
    }
    let isolation = body.isolation_mode.as_deref().unwrap_or("shared");
    if isolation != "shared" && isolation != "strict" {
        return Err(err(StatusCode::BAD_REQUEST, "isolation_mode must be 'shared' or 'strict'"));
    }
    let client_id = ensure_client(&state, &auth.user_id, body.client_id, body.client_name).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let slug = workspace::slugify(name);
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
    let _ = write_matter_md(&state, &id).await?;
    Ok(Json(json!({ "id": id, "name": name, "client_id": client_id, "slug": slug })))
}

async fn get_matter(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    matter_json(&state, &auth.user_id, &id).await
}

async fn update_matter(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<MatterBody>,
) -> ApiResult {
    let isolation = body.isolation_mode.as_deref().unwrap_or("shared");
    if isolation != "shared" && isolation != "strict" {
        return Err(err(StatusCode::BAD_REQUEST, "isolation_mode must be 'shared' or 'strict'"));
    }
    let result = sqlx::query(
        "UPDATE matters SET name = ?, description = ?, isolation_mode = ?, updated_at = datetime('now') \
         WHERE id = ? AND user_id = ?",
    )
    .bind(body.name.trim())
    .bind(&body.description)
    .bind(isolation)
    .bind(&id)
    .bind(&auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Matter not found"));
    }
    let _ = write_matter_md(&state, &id).await?;
    matter_json(&state, &auth.user_id, &id).await
}

async fn delete_matter(
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

pub async fn ensure_default_matter(state: &AppState, user_id: &str) -> Result<(String, String, String), (StatusCode, Json<Value>)> {
    let client_id = ensure_client(state, user_id, None, Some("Unfiled".to_string())).await?;
    let existing: Option<(String, String, String)> = sqlx::query_as(
        "SELECT id, slug, client_id FROM matters WHERE user_id = ? AND slug = '_unfiled'",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if let Some(row) = existing {
        return Ok(row);
    }
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO matters (id, user_id, client_id, name, description, slug, isolation_mode) \
         VALUES (?, ?, ?, 'Unfiled', 'Items pending matter assignment', '_unfiled', 'shared')",
    )
    .bind(&id)
    .bind(user_id)
    .bind(&client_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let _ = write_matter_md(state, &id).await?;
    Ok((id, "_unfiled".to_string(), client_id))
}

pub async fn matter_slug(state: &AppState, user_id: &str, matter_id: &str) -> Result<(String, String), (StatusCode, Json<Value>)> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT slug, client_id FROM matters WHERE id = ? AND user_id = ?",
    )
    .bind(matter_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Matter not found"))
}

async fn ensure_client(
    state: &AppState,
    user_id: &str,
    client_id: Option<String>,
    client_name: Option<String>,
) -> Result<String, (StatusCode, Json<Value>)> {
    if let Some(id) = client_id {
        let exists: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM clients WHERE id = ? AND user_id = ?",
        )
        .bind(&id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        if exists.is_none() {
            return Err(err(StatusCode::NOT_FOUND, "Client not found"));
        }
        return Ok(id);
    }

    let name = client_name.unwrap_or_else(|| "Unfiled".to_string());
    let slug = if name == "Unfiled" { "_unfiled".to_string() } else { workspace::slugify(&name) };
    let existing: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM clients WHERE user_id = ? AND slug = ?",
    )
    .bind(user_id)
    .bind(&slug)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if let Some((id,)) = existing {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO clients (id, user_id, name, slug) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(user_id)
        .bind(&name)
        .bind(&slug)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(id)
}

async fn matter_json(state: &AppState, user_id: &str, id: &str) -> ApiResult {
    let row: Option<(String, String, Option<String>, String, String, String, String, String, String)> =
        sqlx::query_as(
            "SELECT m.id, m.name, m.description, m.slug, m.client_id, c.name, \
                    m.isolation_mode, m.created_at, m.updated_at \
             FROM matters m JOIN clients c ON c.id = m.client_id \
             WHERE m.id = ? AND m.user_id = ?",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (id, name, description, slug, client_id, client_name, isolation_mode, created_at, updated_at) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Matter not found"))?;
    Ok(Json(json!({ "id": id, "name": name, "description": description, "slug": slug,
                    "client_id": client_id, "client_name": client_name,
                    "isolation_mode": isolation_mode,
                    "created_at": created_at, "updated_at": updated_at })))
}

async fn write_matter_md(state: &AppState, id: &str) -> ApiResult {
    let row: (String, String, Option<String>, String, String, String, String, String, String) =
        sqlx::query_as(
            "SELECT m.id, m.name, m.description, m.slug, m.client_id, c.slug, \
                    m.isolation_mode, m.created_at, m.updated_at \
             FROM matters m JOIN clients c ON c.id = m.client_id WHERE m.id = ?",
        )
        .bind(id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let (id, name, description, slug, client_id, client_slug, isolation_mode, created_at, updated_at) = row;
    let dir = if slug == "_unfiled" {
        state.paths.unfiled_matter_dir()
    } else {
        state.paths.matters_dir.join(client_slug).join(&slug)
    };
    std::fs::create_dir_all(dir.join("items"))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    std::fs::create_dir_all(dir.join("attachments"))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let fm = json!({
        "id": id,
        "schema_version": 1,
        "kind": "matter",
        "name": name,
        "slug": slug,
        "client_id": client_id,
        "isolation_mode": isolation_mode,
        "created_at": created_at,
        "updated_at": updated_at,
    });
    let yaml = serde_yaml::to_string(&fm)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let body = description.unwrap_or_default();
    workspace::write_atomic(&dir.join("matter.md"), format!("---\n{yaml}---\n\n{body}\n").as_bytes())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}
