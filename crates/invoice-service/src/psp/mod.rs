use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PspChargeRequest {
    pub card_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PspChargeResponse {
    pub status: String,
    pub psp_ref: Option<String>,
    pub code: Option<String>,
}

#[derive(Debug)]
pub enum PspOutcome {
    Succeeded { psp_ref: String },
    Failed { code: String },
    NetworkError { message: String },
    Timeout,
}

#[derive(Clone)]
pub struct PspClient {
    client: Client,
    url: String,
}

impl PspClient {
    pub fn new(url: String, timeout: Duration) -> Self {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client");
        Self { client, url }
    }

    pub async fn charge(&self, card_token: &str) -> PspOutcome {
        let resp = self
            .client
            .post(&self.url)
            .json(&PspChargeRequest {
                card_token: card_token.to_string(),
            })
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => match r.json::<PspChargeResponse>().await {
                Ok(body) if body.status == "succeeded" => PspOutcome::Succeeded {
                    psp_ref: body.psp_ref.unwrap_or_else(|| "unknown".into()),
                },
                Ok(body) => PspOutcome::Failed {
                    code: body.code.unwrap_or_else(|| "unknown".into()),
                },
                Err(e) => PspOutcome::NetworkError {
                    message: e.to_string(),
                },
            },
            Ok(r) => PspOutcome::NetworkError {
                message: format!("psp status {}", r.status()),
            },
            Err(e) if e.is_timeout() => PspOutcome::Timeout,
            Err(e) => PspOutcome::NetworkError {
                message: e.to_string(),
            },
        }
    }
}
