use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde_json::json;
use std::sync::Arc;

use crate::AppState;
use crate::auth::jwt;

/// The authenticated local user, extracted from a valid session token.
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: String,
    pub username: String,
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let auth = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !auth.starts_with("Bearer ") {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"detail": "Missing or invalid session token"})),
            )
                .into_response());
        }

        let token = auth[7..].trim();
        match jwt::verify_token(token) {
            Ok(claims) => Ok(AuthUser {
                user_id: claims.sub,
                username: claims.email,
            }),
            Err(_) => Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"detail": "Invalid or expired session"})),
            )
                .into_response()),
        }
    }
}
