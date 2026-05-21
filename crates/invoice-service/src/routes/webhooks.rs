use axum::{extract::State, Extension, Json};
use rand::Rng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    error::{AppError, AppResult},
    AppState,
};

#[derive(Deserialize)]
pub struct CreateWebhookEndpointRequest {
    pub url: String,
}

#[derive(Serialize)]
pub struct WebhookEndpointResponse {
    pub id: Uuid,
    pub url: String,
    pub signing_secret: String,
}

#[derive(Serialize)]
pub struct WebhookEndpointListItem {
    pub id: Uuid,
    pub url: String,
}

#[derive(Serialize)]
pub struct ListWebhookEndpointsResponse {
    pub data: Vec<WebhookEndpointListItem>,
}

#[derive(sqlx::FromRow)]
struct WebhookEndpointRow {
    id: Uuid,
    url: String,
}

pub async fn create_webhook_endpoint(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateWebhookEndpointRequest>,
) -> AppResult<Json<WebhookEndpointResponse>> {
    if body.url.trim().is_empty() {
        return Err(AppError::BadRequest("url required".into()));
    }

    let id = Uuid::new_v4();
    let signing_secret: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    sqlx::query(
        r#"
        INSERT INTO webhook_endpoints (id, business_id, url, signing_secret)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(id)
    .bind(auth.business_id)
    .bind(&body.url)
    .bind(&signing_secret)
    .execute(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(Json(WebhookEndpointResponse {
        id,
        url: body.url,
        signing_secret,
    }))
}

pub async fn list_webhook_endpoints(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> AppResult<Json<ListWebhookEndpointsResponse>> {
    let rows = sqlx::query_as::<_, WebhookEndpointRow>(
        r#"
        SELECT id, url FROM webhook_endpoints WHERE business_id = $1 ORDER BY created_at DESC
        "#,
    )
    .bind(auth.business_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(Json(ListWebhookEndpointsResponse {
        data: rows
            .into_iter()
            .map(|r| WebhookEndpointListItem {
                id: r.id,
                url: r.url,
            })
            .collect(),
    }))
}
