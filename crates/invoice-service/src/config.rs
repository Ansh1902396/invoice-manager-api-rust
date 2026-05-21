use std::time::Duration;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub psp_url: String,
    pub psp_timeout: Duration,
    pub psp_reconcile_timeout: Duration,
    pub listen_addr: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/invoices".into()),
            psp_url: std::env::var("PSP_URL")
                .unwrap_or_else(|_| "http://localhost:8081/charge".into()),
            psp_timeout: Duration::from_secs(
                std::env::var("PSP_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5),
            ),
            psp_reconcile_timeout: Duration::from_secs(
                std::env::var("PSP_RECONCILE_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(35),
            ),
            listen_addr: std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
        }
    }
}
