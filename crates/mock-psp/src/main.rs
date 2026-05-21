use axum::{
    extract::Json,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::info;

#[derive(Deserialize)]
struct ChargeRequest {
    card_token: String,
}

#[derive(Serialize)]
struct ChargeResponse {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    psp_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
}

async fn charge(Json(body): Json<ChargeRequest>) -> Response {
    info!(token = %body.card_token, "psp charge request");

    match body.card_token.as_str() {
        "tok_success" | "tok_insufficient_funds" | "tok_card_declined" => {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match body.card_token.as_str() {
                "tok_success" => Json(ChargeResponse {
                    status: "succeeded".into(),
                    psp_ref: Some(uuid::Uuid::new_v4().to_string()),
                    code: None,
                })
                .into_response(),
                "tok_insufficient_funds" => Json(ChargeResponse {
                    status: "failed".into(),
                    psp_ref: None,
                    code: Some("insufficient_funds".into()),
                })
                .into_response(),
                _ => Json(ChargeResponse {
                    status: "failed".into(),
                    psp_ref: None,
                    code: Some("card_declined".into()),
                })
                .into_response(),
            }
        }
        "tok_timeout" => {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Json(ChargeResponse {
                status: "succeeded".into(),
                psp_ref: Some(uuid::Uuid::new_v4().to_string()),
                code: None,
            })
            .into_response()
        }
        "tok_network_error" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        _ => (
            StatusCode::BAD_REQUEST,
            Json(ChargeResponse {
                status: "failed".into(),
                psp_ref: None,
                code: Some("invalid_token".into()),
            }),
        )
            .into_response(),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("mock_psp=info,tower_http=info")
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8081);

    let app = Router::new().route("/charge", post(charge));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("bind");
    info!(port, "mock PSP listening");
    axum::serve(listener, app).await.expect("serve");
}
