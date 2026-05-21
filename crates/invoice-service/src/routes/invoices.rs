use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    domain::{compute_total_cents, InvoiceState},
    error::{AppError, AppResult},
    webhooks,
    AppState,
};

#[derive(Deserialize)]
pub struct LineItemInput {
    pub description: String,
    pub quantity: i64,
    pub unit_amount_cents: i64,
}

#[derive(Deserialize)]
pub struct CreateInvoiceRequest {
    pub customer_id: Uuid,
    pub due_date: NaiveDate,
    pub line_items: Vec<LineItemInput>,
}

#[derive(Serialize)]
pub struct LineItemResponse {
    pub description: String,
    pub quantity: i64,
    pub unit_amount_cents: i64,
}

#[derive(Serialize)]
pub struct InvoiceResponse {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub total_cents: i64,
    pub state: InvoiceState,
    pub due_date: NaiveDate,
    pub line_items: Vec<LineItemResponse>,
}

#[derive(sqlx::FromRow)]
struct InvoiceRow {
    id: Uuid,
    customer_id: Uuid,
    total_cents: i64,
    state: InvoiceState,
    due_date: NaiveDate,
}

#[derive(sqlx::FromRow)]
struct LineItemRow {
    description: String,
    quantity: i64,
    unit_amount_cents: i64,
}

#[derive(sqlx::FromRow)]
struct IdRow {
    id: Uuid,
}

pub async fn create_invoice(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateInvoiceRequest>,
) -> AppResult<Json<InvoiceResponse>> {
    if body.line_items.is_empty() {
        return Err(AppError::BadRequest("line_items required".into()));
    }

    let customer = sqlx::query_as::<_, IdRow>(
        r#"SELECT id FROM customers WHERE id = $1 AND business_id = $2"#,
    )
    .bind(body.customer_id)
    .bind(auth.business_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    .ok_or(AppError::NotFound)?;

    let amounts: Vec<(i64, i64)> = body
        .line_items
        .iter()
        .map(|li| (li.quantity, li.unit_amount_cents))
        .collect();
    let total_cents = compute_total_cents(&amounts).map_err(AppError::BadRequest)?;

    let invoice_id = Uuid::new_v4();
    let mut tx = state.db.begin().await.map_err(|e| AppError::Internal(e.into()))?;

    sqlx::query(
        r#"
        INSERT INTO invoices (id, business_id, customer_id, total_cents, state, due_date)
        VALUES ($1, $2, $3, $4, 'open', $5)
        "#,
    )
    .bind(invoice_id)
    .bind(auth.business_id)
    .bind(customer.id)
    .bind(total_cents)
    .bind(body.due_date)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    let mut line_items = Vec::new();
    for li in &body.line_items {
        sqlx::query(
            r#"
            INSERT INTO invoice_line_items (id, invoice_id, description, quantity, unit_amount_cents)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(invoice_id)
        .bind(&li.description)
        .bind(li.quantity)
        .bind(li.unit_amount_cents)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
        line_items.push(LineItemResponse {
            description: li.description.clone(),
            quantity: li.quantity,
            unit_amount_cents: li.unit_amount_cents,
        });
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let payload = json!({
        "invoice_id": invoice_id,
        "customer_id": body.customer_id,
        "total_cents": total_cents,
        "state": "open",
    });
    webhooks::enqueue_event(&state.db, auth.business_id, "invoice.created", payload)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    Ok(Json(InvoiceResponse {
        id: invoice_id,
        customer_id: body.customer_id,
        total_cents,
        state: InvoiceState::Open,
        due_date: body.due_date,
        line_items,
    }))
}

pub async fn get_invoice(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<InvoiceResponse>> {
    load_invoice(&state, auth.business_id, id).await
}

#[derive(Deserialize)]
pub struct ListInvoicesQuery {
    pub state: Option<InvoiceState>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize)]
pub struct ListInvoicesResponse {
    pub data: Vec<InvoiceResponse>,
}

pub async fn list_invoices(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(q): Query<ListInvoicesQuery>,
) -> AppResult<Json<ListInvoicesResponse>> {
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let offset = q.offset.unwrap_or(0).max(0);

    let rows = if let Some(st) = q.state {
        sqlx::query_as::<_, IdRow>(
            r#"
            SELECT id FROM invoices
            WHERE business_id = $1 AND state = $2
            ORDER BY created_at DESC
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(auth.business_id)
        .bind(st)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, IdRow>(
            r#"
            SELECT id FROM invoices
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
    }
    .map_err(|e| AppError::Internal(e.into()))?;

    let mut data = Vec::new();
    for row in rows {
        data.push(load_invoice(&state, auth.business_id, row.id).await?.0);
    }

    Ok(Json(ListInvoicesResponse { data }))
}

pub async fn void_invoice(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<InvoiceResponse>> {
    let result = sqlx::query_as::<_, IdRow>(
        r#"
        UPDATE invoices SET state = 'void', updated_at = NOW()
        WHERE id = $1 AND business_id = $2 AND state = 'open'
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(auth.business_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    if result.is_none() {
        let current = sqlx::query_as::<_, InvoiceStateRow>(
            r#"SELECT state FROM invoices WHERE id = $1 AND business_id = $2"#,
        )
        .bind(id)
        .bind(auth.business_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

        match current {
            Some(r) if r.state.is_terminal() => {
                return Err(AppError::Conflict(format!(
                    "invoice in terminal state {:?}",
                    r.state
                )));
            }
            Some(_) => {
                return Err(AppError::Conflict(
                    "only open invoices can be voided".into(),
                ));
            }
            None => return Err(AppError::NotFound),
        }
    }

    load_invoice(&state, auth.business_id, id).await
}

#[derive(sqlx::FromRow)]
struct InvoiceStateRow {
    state: InvoiceState,
}

async fn load_invoice(
    state: &AppState,
    business_id: Uuid,
    id: Uuid,
) -> AppResult<Json<InvoiceResponse>> {
    let inv = sqlx::query_as::<_, InvoiceRow>(
        r#"
        SELECT id, customer_id, total_cents, state, due_date
        FROM invoices WHERE id = $1 AND business_id = $2
        "#,
    )
    .bind(id)
    .bind(business_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    .ok_or(AppError::NotFound)?;

    let items = sqlx::query_as::<_, LineItemRow>(
        r#"
        SELECT description, quantity, unit_amount_cents
        FROM invoice_line_items WHERE invoice_id = $1
        "#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    Ok(Json(InvoiceResponse {
        id: inv.id,
        customer_id: inv.customer_id,
        total_cents: inv.total_cents,
        state: inv.state,
        due_date: inv.due_date,
        line_items: items
            .into_iter()
            .map(|li| LineItemResponse {
                description: li.description,
                quantity: li.quantity,
                unit_amount_cents: li.unit_amount_cents,
            })
            .collect(),
    }))
}
