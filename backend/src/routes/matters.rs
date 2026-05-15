//! Matter CRUD route handlers. All SQL lives in
//! `db::repositories::matters` and `db::repositories::clients` per
//! anti-pattern #4. The Markdown-on-disk invariant
//! (`<workspace>/matters/.../matter.md`) is also owned there.
//!
//! A note on `_unfiled`: the docs put `_unfiled` as a sibling of
//! clients (`<workspace>/matters/_unfiled/matter.md`), not nested
//! under a synthetic client. We respect that: the unfiled matter's
//! `client_id` points to a hidden bookkeeping row whose `slug` is
//! also `_unfiled`, so when `write_matter_md` sees `slug == _unfiled`
//! it short-circuits to `paths.unfiled_matter_dir()` and skips the
//! `<client_slug>/<matter_slug>/` join.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{
    auth::middleware::AuthUser,
    db::{models::MatterWithClientRow, repositories},
    workspace, AppState,
};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "detail": msg })))
}

fn into_500<E: std::fmt::Display>(e: E) -> (StatusCode, Json<Value>) {
    err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_matters).post(create_matter))
        .route(
            "/{id}",
            get(get_matter).patch(update_matter).put(update_matter).delete(delete_matter),
        )
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

fn matter_to_json(m: &MatterWithClientRow) -> Value {
    json!({
        "id": m.id,
        "name": m.name,
        "description": m.description,
        "slug": m.slug,
        "client_id": m.client_id,
        "client_name": m.client_name,
        "client_slug": m.client_slug,
        "isolation_mode": m.isolation_mode,
        "created_at": m.created_at,
        "updated_at": m.updated_at,
    })
}

async fn list_matters(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(q): Query<MatterQuery>,
) -> ApiResult {
    let rows = repositories::matters::list_for_user(&state.db, &auth.user_id, q.client_id.as_deref())
        .await
        .map_err(into_500)?;
    Ok(Json(json!({
        "matters": rows.iter().map(matter_to_json).collect::<Vec<_>>()
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
        return Err(err(
            StatusCode::BAD_REQUEST,
            "isolation_mode must be 'shared' or 'strict'",
        ));
    }
    let client_id =
        ensure_client(&state, &auth.user_id, body.client_id, body.client_name).await?;
    let slug = workspace::slugify(name);
    let id = repositories::matters::create(
        &state.db,
        &auth.user_id,
        &client_id,
        name,
        body.description.as_deref(),
        &slug,
        isolation,
    )
    .await
    .map_err(into_500)?;
    write_matter_md(&state, &auth.user_id, &id).await?;
    Ok(Json(json!({
        "id": id, "name": name, "client_id": client_id, "slug": slug
    })))
}

async fn get_matter(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row = repositories::matters::find_by_id(&state.db, &auth.user_id, &id)
        .await
        .map_err(into_500)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Matter not found"))?;
    Ok(Json(matter_to_json(&row)))
}

async fn update_matter(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<MatterBody>,
) -> ApiResult {
    let isolation = body.isolation_mode.as_deref().unwrap_or("shared");
    if isolation != "shared" && isolation != "strict" {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "isolation_mode must be 'shared' or 'strict'",
        ));
    }
    let affected = repositories::matters::update(
        &state.db,
        &auth.user_id,
        &id,
        body.name.trim(),
        body.description.as_deref(),
        isolation,
    )
    .await
    .map_err(into_500)?;
    if affected == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Matter not found"));
    }
    write_matter_md(&state, &auth.user_id, &id).await?;
    let row = repositories::matters::find_by_id(&state.db, &auth.user_id, &id)
        .await
        .map_err(into_500)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Matter not found"))?;
    Ok(Json(matter_to_json(&row)))
}

async fn delete_matter(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let affected = repositories::matters::delete(&state.db, &auth.user_id, &id)
        .await
        .map_err(into_500)?;
    if affected == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Matter not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// Ensure the well-known `_unfiled` matter exists and return
/// `(matter_id, slug, client_id)`. Public for documents/sync handlers.
pub async fn ensure_default_matter(
    state: &AppState,
    user_id: &str,
) -> Result<(String, String, String), (StatusCode, Json<Value>)> {
    if let Some(existing) = repositories::matters::find_unfiled(&state.db, user_id)
        .await
        .map_err(into_500)?
    {
        return Ok((existing.id, existing.slug, existing.client_id));
    }
    let client_id = ensure_client(state, user_id, None, Some("Unfiled".into())).await?;
    let id = repositories::matters::create(
        &state.db,
        user_id,
        &client_id,
        "Unfiled",
        Some("Items pending matter assignment"),
        "_unfiled",
        "shared",
    )
    .await
    .map_err(into_500)?;
    write_matter_md(state, user_id, &id).await?;
    Ok((id, "_unfiled".to_string(), client_id))
}

/// Lightweight lookup used by other route modules.
pub async fn matter_slug(
    state: &AppState,
    user_id: &str,
    matter_id: &str,
) -> Result<(String, String), (StatusCode, Json<Value>)> {
    repositories::matters::find_slug_by_id(&state.db, user_id, matter_id)
        .await
        .map_err(into_500)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Matter not found"))
}

async fn ensure_client(
    state: &AppState,
    user_id: &str,
    client_id: Option<String>,
    client_name: Option<String>,
) -> Result<String, (StatusCode, Json<Value>)> {
    if let Some(id) = client_id {
        let exists = repositories::clients::find_by_id(&state.db, user_id, &id)
            .await
            .map_err(into_500)?;
        if exists.is_none() {
            return Err(err(StatusCode::NOT_FOUND, "Client not found"));
        }
        return Ok(id);
    }
    let name = client_name.unwrap_or_else(|| "Unfiled".to_string());
    let slug = if name == "Unfiled" {
        "_unfiled".to_string()
    } else {
        workspace::slugify(&name)
    };
    if let Some(existing) = repositories::clients::find_by_slug(&state.db, user_id, &slug)
        .await
        .map_err(into_500)?
    {
        return Ok(existing.id);
    }
    let id = repositories::clients::create(&state.db, user_id, &name, &slug, None)
        .await
        .map_err(into_500)?;
    Ok(id)
}

async fn write_matter_md(
    state: &AppState,
    user_id: &str,
    id: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    let row = repositories::matters::find_by_id(&state.db, user_id, id)
        .await
        .map_err(into_500)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Matter not found"))?;
    repositories::matters::write_matter_md(&state.paths, &row).map_err(into_500)
}
