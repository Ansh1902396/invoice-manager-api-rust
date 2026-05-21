use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{error::AppError, AppState};

#[derive(Clone, Debug)]
pub struct AuthContext {
    pub business_id: Uuid,
    pub api_key_id: Uuid,
}

#[derive(sqlx::FromRow)]
struct ApiKeyRow {
    id: Uuid,
    business_id: Uuid,
}

pub fn hash_api_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn api_key_prefix(key: &str) -> String {
    key.chars().take(8).collect()
}

pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(AppError::Unauthorized)?;

    let ctx = authenticate(&state.db, token).await?;
    req.extensions_mut().insert(ctx);
    Ok(next.run(req).await)
}

async fn authenticate(pool: &PgPool, token: &str) -> Result<AuthContext, AppError> {
    let prefix = api_key_prefix(token);
    let hash = hash_api_key(token);

    let row = sqlx::query_as::<_, ApiKeyRow>(
        r#"
        SELECT id, business_id
        FROM api_keys
        WHERE key_prefix = $1 AND key_hash = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(prefix)
    .bind(hash)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    .ok_or(AppError::Unauthorized)?;

    Ok(AuthContext {
        business_id: row.business_id,
        api_key_id: row.id,
    })
}
