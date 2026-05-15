//! Typed structs returned by `db::repositories::*`.
//!
//! Routes import these instead of building anonymous tuples from
//! `sqlx::query_as`. They live here (rather than next to each
//! repository module) so the same struct can be returned from
//! multiple repositories without circular imports.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ClientRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub slug: String,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MatterRow {
    pub id: String,
    pub user_id: String,
    pub client_id: String,
    pub name: String,
    pub description: Option<String>,
    pub slug: String,
    pub isolation_mode: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Joined matter + client display name. The `clients.name` is what
/// the UI renders next to the matter; without the join the route
/// would have to make a second query per matter.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MatterWithClientRow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub slug: String,
    pub client_id: String,
    pub client_name: String,
    pub client_slug: String,
    pub isolation_mode: String,
    pub created_at: String,
    pub updated_at: String,
}
