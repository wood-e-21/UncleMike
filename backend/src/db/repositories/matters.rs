//! Matter CRUD. Owns the `matters` table and the on-disk
//! `matter.md` invariant: every successful write to the table is
//! followed by a successful atomic write to
//! `<workspace>/matters/<client>/<matter>/matter.md` (or
//! `_unfiled/matter.md` for the unfiled tray).

use anyhow::Result;
use serde_json::json;
use sqlx::SqlitePool;

use crate::db::models::{MatterRow, MatterWithClientRow};
use crate::workspace::{self, WorkspacePaths};

pub async fn list_for_user(
    pool: &SqlitePool,
    user_id: &str,
    client_id_filter: Option<&str>,
) -> Result<Vec<MatterWithClientRow>> {
    let rows = if let Some(client_id) = client_id_filter {
        sqlx::query_as::<_, MatterWithClientRow>(
            "SELECT m.id, m.name, m.description, m.slug, m.client_id, \
                    c.name as client_name, c.slug as client_slug, \
                    m.isolation_mode, m.created_at, m.updated_at \
             FROM matters m JOIN clients c ON c.id = m.client_id \
             WHERE m.user_id = ? AND m.client_id = ? \
             ORDER BY m.updated_at DESC",
        )
        .bind(user_id)
        .bind(client_id)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, MatterWithClientRow>(
            "SELECT m.id, m.name, m.description, m.slug, m.client_id, \
                    c.name as client_name, c.slug as client_slug, \
                    m.isolation_mode, m.created_at, m.updated_at \
             FROM matters m JOIN clients c ON c.id = m.client_id \
             WHERE m.user_id = ? ORDER BY m.updated_at DESC",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?
    };
    Ok(rows)
}

pub async fn find_by_id(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
) -> Result<Option<MatterWithClientRow>> {
    let row = sqlx::query_as::<_, MatterWithClientRow>(
        "SELECT m.id, m.name, m.description, m.slug, m.client_id, \
                c.name as client_name, c.slug as client_slug, \
                m.isolation_mode, m.created_at, m.updated_at \
         FROM matters m JOIN clients c ON c.id = m.client_id \
         WHERE m.id = ? AND m.user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn find_slug_by_id(
    pool: &SqlitePool,
    user_id: &str,
    matter_id: &str,
) -> Result<Option<(String, String)>> {
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT slug, client_id FROM matters WHERE id = ? AND user_id = ?",
    )
    .bind(matter_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Look up an existing `_unfiled` matter for the given user, if any.
/// Used by `ensure_unfiled` to dedupe.
pub async fn find_unfiled(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<MatterRow>> {
    let row = sqlx::query_as::<_, MatterRow>(
        "SELECT id, user_id, client_id, name, description, slug, isolation_mode, \
                created_at, updated_at \
         FROM matters WHERE user_id = ? AND slug = '_unfiled'",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn create(
    pool: &SqlitePool,
    user_id: &str,
    client_id: &str,
    name: &str,
    description: Option<&str>,
    slug: &str,
    isolation_mode: &str,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO matters (id, user_id, client_id, name, description, slug, isolation_mode) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(client_id)
    .bind(name)
    .bind(description)
    .bind(slug)
    .bind(isolation_mode)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn update(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
    name: &str,
    description: Option<&str>,
    isolation_mode: &str,
) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE matters SET name = ?, description = ?, isolation_mode = ?, \
                            updated_at = datetime('now') \
         WHERE id = ? AND user_id = ?",
    )
    .bind(name)
    .bind(description)
    .bind(isolation_mode)
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

pub async fn delete(pool: &SqlitePool, user_id: &str, id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM matters WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Write the on-disk `matter.md` for a matter. Path:
///   - `<workspace>/matters/_unfiled/matter.md`            (slug == _unfiled)
///   - `<workspace>/matters/<client_slug>/<slug>/matter.md`  (otherwise)
///
/// Atomic via tmp-then-rename per docs/01-workspace-layout.md.
pub fn write_matter_md(
    paths: &WorkspacePaths,
    matter: &MatterWithClientRow,
) -> Result<()> {
    let dir = if matter.slug == "_unfiled" {
        paths.unfiled_matter_dir()
    } else {
        paths
            .matters_dir
            .join(&matter.client_slug)
            .join(&matter.slug)
    };
    std::fs::create_dir_all(dir.join("items"))?;
    std::fs::create_dir_all(dir.join("attachments"))?;
    let fm = json!({
        "id": matter.id,
        "schema_version": 1,
        "kind": "matter",
        "name": matter.name,
        "slug": matter.slug,
        "client_id": matter.client_id,
        "isolation_mode": matter.isolation_mode,
        "created_at": matter.created_at,
        "updated_at": matter.updated_at,
    });
    let yaml = serde_yaml::to_string(&fm)?;
    let body = matter.description.clone().unwrap_or_default();
    let contents = format!("---\n{yaml}---\n\n{body}\n");
    workspace::write_atomic(&dir.join("matter.md"), contents.as_bytes())?;
    Ok(())
}
