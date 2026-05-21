use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    error::{AppError, AppResult},
    AppState,
};

#[derive(Deserialize)]
pub struct CreateCustomerRequest {
    pub name: String,
    pub email: String,
}

#[derive(Serialize)]
pub struct CustomerResponse {
    pub id: Uuid,
    pub name: String,
    pub email: String,
}

pub async fn create_customer(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateCustomerRequest>,
) -> AppResult<Json<CustomerResponse>> {
    if body.name.trim().is_empty() || body.email.trim().is_empty() {
        return Err(AppError::BadRequest("name and email required".into()));
    }

    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO customers (id, business_id, name, email)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(id)
    .bind(auth.business_id)
    .bind(&body.name)
    .bind(&body.email)
    .execute(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(Json(CustomerResponse {
        id,
        name: body.name,
        email: body.email,
    }))
}

#[derive(sqlx::FromRow)]
struct CustomerRow {
    id: Uuid,
    name: String,
    email: String,
}

pub async fn get_customer(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<CustomerResponse>> {
    let row = sqlx::query_as::<_, CustomerRow>(
        r#"
        SELECT id, name, email FROM customers
        WHERE id = $1 AND business_id = $2
        "#,
    )
    .bind(id)
    .bind(auth.business_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    .ok_or(AppError::NotFound)?;

    Ok(Json(CustomerResponse {
        id: row.id,
        name: row.name,
        email: row.email,
    }))
}

#[derive(Deserialize)]
pub struct ListCustomersQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize)]
pub struct ListCustomersResponse {
    pub data: Vec<CustomerResponse>,
}

pub async fn list_customers(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(q): Query<ListCustomersQuery>,
) -> AppResult<Json<ListCustomersResponse>> {
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let offset = q.offset.unwrap_or(0).max(0);

    let rows = sqlx::query_as::<_, CustomerRow>(
        r#"
        SELECT id, name, email FROM customers
        WHERE business_id = $1
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(auth.business_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(Json(ListCustomersResponse {
        data: rows
            .into_iter()
            .map(|r| CustomerResponse {
                id: r.id,
                name: r.name,
                email: r.email,
            })
            .collect(),
    }))
}
