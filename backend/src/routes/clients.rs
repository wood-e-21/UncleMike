//! Client CRUD route handlers. SQL lives in `db::repositories::clients`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{
    auth::middleware::AuthUser,
    db::{models::ClientRow, repositories},
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
        .route("/", get(list_clients).post(create_client))
        .route(
            "/{id}",
            get(get_client).patch(update_client).put(update_client).delete(delete_client),
        )
}

#[derive(Deserialize)]
struct ClientBody {
    name: String,
    notes: Option<String>,
}

fn client_to_json(c: &ClientRow) -> Value {
    json!({
        "id": c.id,
        "name": c.name,
        "slug": c.slug,
        "notes": c.notes,
        "created_at": c.created_at,
        "updated_at": c.updated_at,
    })
}

async fn list_clients(State(state): State<Arc<AppState>>, auth: AuthUser) -> ApiResult {
    let rows = repositories::clients::list_for_user(&state.db, &auth.user_id)
        .await
        .map_err(into_500)?;
    Ok(Json(json!({
        "clients": rows.iter().map(client_to_json).collect::<Vec<_>>()
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
    let slug = workspace::slugify(name);
    let id = repositories::clients::create(
        &state.db,
        &auth.user_id,
        name,
        &slug,
        body.notes.as_deref(),
    )
    .await
    .map_err(into_500)?;
    write_client_md(&state, &auth.user_id, &id).await?;
    Ok(Json(json!({ "id": id, "name": name, "slug": slug })))
}

async fn get_client(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row = repositories::clients::find_by_id(&state.db, &auth.user_id, &id)
        .await
        .map_err(into_500)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Client not found"))?;
    Ok(Json(client_to_json(&row)))
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
    let affected = repositories::clients::update(
        &state.db,
        &auth.user_id,
        &id,
        name,
        body.notes.as_deref(),
    )
    .await
    .map_err(into_500)?;
    if affected == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Client not found"));
    }
    write_client_md(&state, &auth.user_id, &id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_client(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let affected = repositories::clients::delete(&state.db, &auth.user_id, &id)
        .await
        .map_err(into_500)?;
    if affected == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Client not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn write_client_md(
    state: &AppState,
    user_id: &str,
    id: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    let row = repositories::clients::find_by_id(&state.db, user_id, id)
        .await
        .map_err(into_500)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Client not found"))?;
    repositories::clients::write_client_md(&state.paths, &row).map_err(into_500)
}
