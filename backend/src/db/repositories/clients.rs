//! Client CRUD. The owning repository for the `clients` table.

use anyhow::Result;
use sqlx::SqlitePool;

use serde_json::json;

use crate::db::models::ClientRow;
use crate::workspace::{self, WorkspacePaths};

pub async fn list_for_user(pool: &SqlitePool, user_id: &str) -> Result<Vec<ClientRow>> {
    let rows = sqlx::query_as::<_, ClientRow>(
        "SELECT id, user_id, name, slug, notes, created_at, updated_at \
         FROM clients WHERE user_id = ? ORDER BY name ASC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn find_by_id(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
) -> Result<Option<ClientRow>> {
    let row = sqlx::query_as::<_, ClientRow>(
        "SELECT id, user_id, name, slug, notes, created_at, updated_at \
         FROM clients WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn find_by_slug(
    pool: &SqlitePool,
    user_id: &str,
    slug: &str,
) -> Result<Option<ClientRow>> {
    let row = sqlx::query_as::<_, ClientRow>(
        "SELECT id, user_id, name, slug, notes, created_at, updated_at \
         FROM clients WHERE user_id = ? AND slug = ?",
    )
    .bind(user_id)
    .bind(slug)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn create(
    pool: &SqlitePool,
    user_id: &str,
    name: &str,
    slug: &str,
    notes: Option<&str>,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO clients (id, user_id, name, slug, notes) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(name)
    .bind(slug)
    .bind(notes)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn update(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
    name: &str,
    notes: Option<&str>,
) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE clients SET name = ?, notes = ?, updated_at = datetime('now') \
         WHERE id = ? AND user_id = ?",
    )
    .bind(name)
    .bind(notes)
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

pub async fn delete(pool: &SqlitePool, user_id: &str, id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM clients WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Write `<workspace>/matters/<slug>/client.md`. Atomic.
pub fn write_client_md(paths: &WorkspacePaths, client: &ClientRow) -> Result<()> {
    let dir = paths.matters_dir.join(&client.slug);
    std::fs::create_dir_all(&dir)?;
    let frontmatter = json!({
        "id": client.id,
        "schema_version": 1,
        "kind": "client",
        "name": client.name,
        "slug": client.slug,
        "created_at": client.created_at,
        "updated_at": client.updated_at,
    });
    let yaml = serde_yaml::to_string(&frontmatter)?;
    let body = client.notes.clone().unwrap_or_default();
    let contents = format!("---\n{yaml}---\n\n{body}\n");
    workspace::write_atomic(&dir.join("client.md"), contents.as_bytes())?;
    Ok(())
}
