use axum::Router;
use invoice_service::{build_router, config::Config, connect_pool, make_state};
use std::sync::OnceLock;

pub type TestApp = Router;

pub fn api_key_header() -> &'static str {
    "Bearer dodo_live_demokey1234567890123456789012"
}

static INIT: OnceLock<()> = OnceLock::new();

pub async fn setup_test_app() -> (TestApp, sqlx::PgPool) {
    INIT.get_or_init(|| {
        std::env::set_var(
            "DATABASE_URL",
            std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:5432/invoices".into()),
        );
        std::env::set_var(
            "PSP_URL",
            std::env::var("PSP_URL").unwrap_or_else(|_| "http://127.0.0.1:8081/charge".into()),
        );
    });

    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = connect_pool(&url)
        .await
        .expect("connect test database — run docker compose up first");

    let config = Config::from_env();
    let state = make_state(pool.clone(), &config);
    (build_router(state), pool)
}
