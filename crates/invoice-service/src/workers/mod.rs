use crate::{
    domain::{InvoiceState, PaymentAttemptStatus},
    psp::{PspClient, PspOutcome},
    webhooks,
};
use serde_json::json;
use sqlx::PgPool;
use tracing::{error, info};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct PendingAttemptRow {
    id: Uuid,
    business_id: Uuid,
    invoice_id: Uuid,
    card_token: String,
}

pub async fn run_payment_reconciler(pool: PgPool, psp: PspClient) {
    loop {
        if let Err(e) = reconcile_pending(&pool, &psp).await {
            error!(error = %e, "payment reconciler tick failed");
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

async fn reconcile_pending(pool: &PgPool, psp: &PspClient) -> anyhow::Result<()> {
    let rows = sqlx::query_as::<_, PendingAttemptRow>(
        r#"
        SELECT id, business_id, invoice_id, card_token
        FROM payment_attempts
        WHERE status = 'pending'
        ORDER BY created_at
        LIMIT 10
        "#,
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let outcome = psp.charge(&row.card_token).await;
        finalize_attempt(pool, row.id, row.business_id, row.invoice_id, outcome).await?;
    }

    Ok(())
}

pub async fn finalize_attempt(
    pool: &PgPool,
    attempt_id: Uuid,
    business_id: Uuid,
    invoice_id: Uuid,
    outcome: PspOutcome,
) -> anyhow::Result<()> {
    match outcome {
        PspOutcome::Succeeded { psp_ref } => {
            let mut tx = pool.begin().await?;
            sqlx::query(
                r#"
                UPDATE payment_attempts
                SET status = 'succeeded', psp_ref = $2, updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(attempt_id)
            .bind(&psp_ref)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                r#"
                UPDATE invoices SET state = 'paid', updated_at = NOW()
                WHERE id = $1 AND state = 'open'
                "#,
            )
            .bind(invoice_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;

            let payload = json!({
                "invoice_id": invoice_id,
                "payment_attempt_id": attempt_id,
                "psp_ref": psp_ref,
            });
            webhooks::enqueue_event(pool, business_id, "invoice.paid", payload).await?;
            info!(%attempt_id, %invoice_id, "payment succeeded via reconciler");
        }
        PspOutcome::Failed { code } => {
            sqlx::query(
                r#"
                UPDATE payment_attempts
                SET status = 'failed', failure_code = $2, updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(attempt_id)
            .bind(&code)
            .execute(pool)
            .await?;

            let payload = json!({
                "invoice_id": invoice_id,
                "payment_attempt_id": attempt_id,
                "failure_code": code,
            });
            webhooks::enqueue_event(pool, business_id, "invoice.payment_failed", payload).await?;
        }
        PspOutcome::Timeout | PspOutcome::NetworkError { .. } => {}
    }

    Ok(())
}

pub async fn apply_psp_outcome_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    attempt_id: Uuid,
    invoice_id: Uuid,
    outcome: PspOutcome,
) -> anyhow::Result<(PaymentAttemptStatus, Option<InvoiceState>, Option<serde_json::Value>)> {
    match outcome {
        PspOutcome::Succeeded { psp_ref } => {
            sqlx::query(
                r#"
                UPDATE payment_attempts
                SET status = 'succeeded', psp_ref = $2, updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(attempt_id)
            .bind(&psp_ref)
            .execute(&mut **tx)
            .await?;

            sqlx::query(
                r#"
                UPDATE invoices SET state = 'paid', updated_at = NOW()
                WHERE id = $1 AND state = 'open'
                "#,
            )
            .bind(invoice_id)
            .execute(&mut **tx)
            .await?;

            let webhook = json!({
                "invoice_id": invoice_id,
                "payment_attempt_id": attempt_id,
                "psp_ref": psp_ref,
            });
            Ok((PaymentAttemptStatus::Succeeded, Some(InvoiceState::Paid), Some(webhook)))
        }
        PspOutcome::Failed { code } => {
            sqlx::query(
                r#"
                UPDATE payment_attempts
                SET status = 'failed', failure_code = $2, updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(attempt_id)
            .bind(&code)
            .execute(&mut **tx)
            .await?;

            let webhook = json!({
                "invoice_id": invoice_id,
                "payment_attempt_id": attempt_id,
                "failure_code": code,
            });
            Ok((PaymentAttemptStatus::Failed, None, Some(webhook)))
        }
        PspOutcome::Timeout | PspOutcome::NetworkError { .. } => {
            sqlx::query(
                r#"
                UPDATE payment_attempts SET status = 'pending', updated_at = NOW() WHERE id = $1
                "#,
            )
            .bind(attempt_id)
            .execute(&mut **tx)
            .await?;
            Ok((PaymentAttemptStatus::Pending, None, None))
        }
    }
}
