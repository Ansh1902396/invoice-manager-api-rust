mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::{api_key_header, setup_test_app};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn concurrent_pay_only_one_succeeds() {
    let (app, _pool) = setup_test_app().await;
    let customer = create_customer(&app).await;
    let invoice = create_invoice(&app, customer).await;
    let invoice_id = invoice["id"].as_str().unwrap();

    let mut handles = Vec::new();
    for i in 0..10 {
        let app = app.clone();
        let invoice_id = invoice_id.to_string();
        handles.push(tokio::spawn(async move {
            let body = json!({ "card_token": "tok_success" });
            let req = Request::builder()
                .method("POST")
                .uri(format!("/v1/invoices/{invoice_id}/pay"))
                .header("Authorization", api_key_header())
                .header("Idempotency-Key", format!("concurrent-{invoice_id}-{i}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            app.clone().oneshot(req).await.unwrap()
        }));
    }

    let mut ok_count = 0;
    let mut conflict_count = 0;
    for handle in handles {
        let resp = handle.await.unwrap();
        match resp.status() {
            StatusCode::OK => ok_count += 1,
            StatusCode::CONFLICT => conflict_count += 1,
            other => panic!("unexpected status {other}"),
        }
    }

    assert_eq!(ok_count, 1, "exactly one payment should succeed");
    assert_eq!(
        conflict_count, 9,
        "other concurrent attempts should conflict"
    );

    let invoice = get_invoice(&app, invoice_id).await;
    assert_eq!(invoice["state"], "paid");
}

#[tokio::test]
async fn idempotency_replays_without_double_charge() {
    let (app, pool) = setup_test_app().await;
    let customer = create_customer(&app).await;
    let invoice = create_invoice(&app, customer).await;
    let invoice_id = invoice["id"].as_str().unwrap();
    let key = format!("idem-test-key-{invoice_id}");

    let first = pay(&app, invoice_id, "tok_success", &key).await;
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = body_json(first).await;

    let second = pay(&app, invoice_id, "tok_success", &key).await;
    assert_eq!(second.status(), StatusCode::OK);
    let second_body = body_json(second).await;
    assert_eq!(first_body, second_body);

    let attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM payment_attempts WHERE invoice_id = $1 AND status = 'succeeded'",
    )
    .bind(Uuid::parse_str(invoice_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn psp_network_error_does_not_corrupt_invoice() {
    let (app, pool) = setup_test_app().await;
    let customer = create_customer(&app).await;
    let invoice = create_invoice(&app, customer).await;
    let invoice_id = invoice["id"].as_str().unwrap();
    let key = format!("net-err-key-{invoice_id}");

    let start = Instant::now();
    let resp = pay(&app, invoice_id, "tok_network_error", &key).await;
    assert!(
        start.elapsed() < Duration::from_secs(6),
        "endpoint must not hang on network error"
    );
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let invoice = get_invoice(&app, invoice_id).await;
    assert_eq!(invoice["state"], "open");

    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM payment_attempts WHERE invoice_id = $1 AND status = 'pending'",
    )
    .bind(Uuid::parse_str(invoice_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 1);
}

async fn create_customer(app: &common::TestApp) -> Value {
    let body = json!({ "name": "Test Co", "email": "test@example.com" });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/customers")
        .header("Authorization", api_key_header())
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp).await
}

async fn create_invoice(app: &common::TestApp, customer: Value) -> Value {
    let body = json!({
        "customer_id": customer["id"],
        "due_date": "2026-12-31",
        "line_items": [{
            "description": "Widget",
            "quantity": 2,
            "unit_amount_cents": 500
        }]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/invoices")
        .header("Authorization", api_key_header())
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp).await
}

async fn get_invoice(app: &common::TestApp, invoice_id: &str) -> Value {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/invoices/{invoice_id}"))
        .header("Authorization", api_key_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp).await
}

async fn pay(
    app: &common::TestApp,
    invoice_id: &str,
    token: &str,
    idempotency_key: &str,
) -> axum::response::Response {
    let body = json!({ "card_token": token });
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/invoices/{invoice_id}/pay"))
        .header("Authorization", api_key_header())
        .header("Idempotency-Key", idempotency_key)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
