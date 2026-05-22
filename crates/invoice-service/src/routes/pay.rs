use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    domain::{InvoiceState, PaymentAttemptStatus},
    error::{AppError, AppResult},
    webhooks, workers, AppState,
};

#[derive(Deserialize, Serialize)]
pub struct PayInvoiceRequest {
    pub card_token: String,
}

#[derive(sqlx::FromRow)]
struct ExistingAttemptRow {
    request_body_hash: String,
    response_status_code: Option<i32>,
    response_body: Option<Value>,
}

#[derive(sqlx::FromRow)]
struct InvoiceLockRow {
    state: InvoiceState,
}

fn hash_body(body: &PayInvoiceRequest) -> String {
    let raw = serde_json::to_string(body).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

pub async fn pay_invoice(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(invoice_id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<PayInvoiceRequest>,
) -> AppResult<Response> {
    let idempotency_key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::BadRequest("Idempotency-Key header required".into()))?;

    let body_hash = hash_body(&body);

    if let Some(existing) = sqlx::query_as::<_, ExistingAttemptRow>(
        r#"
        SELECT request_body_hash, response_status_code, response_body
        FROM payment_attempts
        WHERE business_id = $1 AND idempotency_key = $2
        "#,
    )
    .bind(auth.business_id)
    .bind(idempotency_key)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    {
        if existing.request_body_hash != body_hash {
            return Err(AppError::Unprocessable(
                "idempotency key reused with different request body".into(),
            ));
        }
        return Ok(replay_response(
            existing.response_status_code.unwrap_or(200),
            existing.response_body,
        ));
    }

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let invoice = match sqlx::query_as::<_, InvoiceLockRow>(
        r#"
        SELECT state
        FROM invoices
        WHERE id = $1 AND business_id = $2
        FOR UPDATE
        "#,
    )
    .bind(invoice_id)
    .bind(auth.business_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    {
        Some(invoice) => invoice,
        None => {
            tx.rollback()
                .await
                .map_err(|e| AppError::Internal(e.into()))?;
            return Err(AppError::NotFound);
        }
    };

    if !invoice.state.can_pay() {
        tx.rollback()
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        return Err(AppError::Conflict(format!(
            "invoice is not payable in state {:?}",
            invoice.state
        )));
    }

    let in_flight = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM payment_attempts
        WHERE invoice_id = $1 AND status IN ('processing', 'pending')
        LIMIT 1
        "#,
    )
    .bind(invoice_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    if in_flight.is_some() {
        tx.rollback()
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
        return Err(AppError::Conflict("payment already in progress".into()));
    }

    let attempt_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO payment_attempts
            (id, business_id, invoice_id, idempotency_key, request_body_hash, card_token, status)
        VALUES ($1, $2, $3, $4, $5, $6, 'processing')
        "#,
    )
    .bind(attempt_id)
    .bind(auth.business_id)
    .bind(invoice_id)
    .bind(idempotency_key)
    .bind(&body_hash)
    .bind(&body.card_token)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let outcome = state.psp.charge(&body.card_token).await;

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    let (status, invoice_state, webhook_payload) =
        workers::apply_psp_outcome_in_tx(&mut tx, attempt_id, invoice_id, outcome)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;

    let (http_status, response_body) =
        build_pay_response(attempt_id, invoice_id, status, invoice_state);

    sqlx::query(
        r#"
        UPDATE payment_attempts
        SET response_status_code = $2, response_body = $3, updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(attempt_id)
    .bind(http_status.as_u16() as i32)
    .bind(&response_body)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

    if let Some(payload) = webhook_payload {
        let event = if status == PaymentAttemptStatus::Succeeded {
            "invoice.paid"
        } else {
            "invoice.payment_failed"
        };
        webhooks::enqueue_event(&state.db, auth.business_id, event, payload)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
    }

    Ok((http_status, Json(response_body)).into_response())
}

fn build_pay_response(
    attempt_id: Uuid,
    invoice_id: Uuid,
    status: PaymentAttemptStatus,
    invoice_state: Option<InvoiceState>,
) -> (StatusCode, Value) {
    let attempt = json!({
        "id": attempt_id,
        "invoice_id": invoice_id,
        "status": status,
    });

    match status {
        PaymentAttemptStatus::Succeeded => (
            StatusCode::OK,
            json!({ "payment_attempt": attempt, "invoice_state": invoice_state }),
        ),
        PaymentAttemptStatus::Failed => (
            StatusCode::PAYMENT_REQUIRED,
            json!({ "payment_attempt": attempt, "invoice_state": InvoiceState::Open }),
        ),
        PaymentAttemptStatus::Pending | PaymentAttemptStatus::Processing => (
            StatusCode::ACCEPTED,
            json!({
                "payment_attempt": attempt,
                "invoice_state": InvoiceState::Open,
                "message": "payment pending; poll GET /invoices/{id}"
            }),
        ),
    }
}

fn replay_response(status_code: i32, body: Option<Value>) -> Response {
    let status = StatusCode::from_u16(status_code as u16).unwrap_or(StatusCode::OK);
    (status, Json(body.unwrap_or(json!({})))).into_response()
}
