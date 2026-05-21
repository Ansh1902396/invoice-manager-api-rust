use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub const RETRY_INTERVALS_SECS: [i64; 6] = [0, 30, 120, 600, 3600, 21600];

#[derive(sqlx::FromRow)]
struct DueDeliveryRow {
    id: Uuid,
    business_id: Uuid,
    endpoint_id: Uuid,
    event_type: String,
    payload: Value,
    attempt_count: i32,
    url: String,
    signing_secret: String,
}

pub fn sign_payload(secret: &str, timestamp: i64, body: &str) -> String {
    let signed = format!("{timestamp}.{body}");
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(signed.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub async fn enqueue_event(
    pool: &PgPool,
    business_id: Uuid,
    event_type: &str,
    payload: Value,
) -> anyhow::Result<()> {
    let endpoints = sqlx::query_as::<_, EndpointIdRow>(
        r#"SELECT id FROM webhook_endpoints WHERE business_id = $1"#,
    )
    .bind(business_id)
    .fetch_all(pool)
    .await?;

    for ep in endpoints {
        sqlx::query(
            r#"
            INSERT INTO webhook_deliveries (id, business_id, endpoint_id, event_type, payload, status, next_retry_at)
            VALUES ($1, $2, $3, $4, $5, 'pending', NOW())
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(business_id)
        .bind(ep.id)
        .bind(event_type)
        .bind(&payload)
        .execute(pool)
        .await?;
    }

    Ok(())
}

#[derive(sqlx::FromRow)]
struct EndpointIdRow {
    id: Uuid,
}

pub async fn run_webhook_worker(pool: PgPool) {
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("webhook client");

    loop {
        if let Err(e) = process_due_deliveries(&pool, &client).await {
            error!(error = %e, "webhook worker tick failed");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn process_due_deliveries(pool: &PgPool, client: &Client) -> anyhow::Result<()> {
    let rows = sqlx::query_as::<_, DueDeliveryRow>(
        r#"
        SELECT d.id, d.business_id, d.endpoint_id, d.event_type, d.payload,
               d.attempt_count, e.url, e.signing_secret
        FROM webhook_deliveries d
        JOIN webhook_endpoints e ON e.id = d.endpoint_id
        WHERE d.status = 'pending' AND d.next_retry_at <= NOW()
        ORDER BY d.next_retry_at
        LIMIT 20
        "#,
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let body = json!({
            "id": row.id,
            "type": row.event_type,
            "created_at": Utc::now(),
            "data": row.payload,
        });
        let body_str = serde_json::to_string(&body)?;
        let timestamp = Utc::now().timestamp();
        let signature = sign_payload(&row.signing_secret, timestamp, &body_str);

        let result = client
            .post(&row.url)
            .header("Content-Type", "application/json")
            .header("X-Webhook-Id", row.id.to_string())
            .header("X-Webhook-Timestamp", timestamp.to_string())
            .header("X-Webhook-Signature", signature)
            .body(body_str)
            .send()
            .await;

        let attempt = row.attempt_count + 1;
        match result {
            Ok(resp) if resp.status().is_success() => {
                sqlx::query(
                    r#"
                    UPDATE webhook_deliveries
                    SET status = 'delivered', attempt_count = $2, updated_at = NOW(), last_error = NULL
                    WHERE id = $1
                    "#,
                )
                .bind(row.id)
                .bind(attempt)
                .execute(pool)
                .await?;
                info!(delivery_id = %row.id, "webhook delivered");
            }
            Ok(resp) => {
                let err = format!("HTTP {}", resp.status());
                schedule_retry(pool, row.id, attempt, &err).await?;
            }
            Err(e) => {
                schedule_retry(pool, row.id, attempt, &e.to_string()).await?;
            }
        }
    }

    Ok(())
}

async fn schedule_retry(
    pool: &PgPool,
    id: Uuid,
    attempt: i32,
    err: &str,
) -> anyhow::Result<()> {
    let idx = (attempt as usize).saturating_sub(1);
    if idx >= RETRY_INTERVALS_SECS.len() {
        sqlx::query(
            r#"
            UPDATE webhook_deliveries
            SET status = 'dead', attempt_count = $2, updated_at = NOW(), last_error = $3
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(attempt)
        .bind(err)
        .execute(pool)
        .await?;
        warn!(delivery_id = %id, "webhook delivery exhausted retries");
    } else {
        let delay = RETRY_INTERVALS_SECS[idx];
        let next: DateTime<Utc> = Utc::now() + chrono::Duration::seconds(delay);
        sqlx::query(
            r#"
            UPDATE webhook_deliveries
            SET attempt_count = $2, next_retry_at = $3, updated_at = NOW(), last_error = $4
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(attempt)
        .bind(next)
        .bind(err)
        .execute(pool)
        .await?;
    }
    Ok(())
}
