pub mod auth;
pub mod config;
pub mod domain;
pub mod error;
pub mod psp;
pub mod routes;
pub mod webhooks;
pub mod workers;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

use auth::auth_middleware;
use config::Config;
use psp::PspClient;

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub psp: Arc<PspClient>,
    pub psp_reconcile: Arc<PspClient>,
}

pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route(
            "/customers",
            post(routes::customers::create_customer).get(routes::customers::list_customers),
        )
        .route("/customers/{id}", get(routes::customers::get_customer))
        .route(
            "/invoices",
            post(routes::invoices::create_invoice).get(routes::invoices::list_invoices),
        )
        .route("/invoices/{id}", get(routes::invoices::get_invoice))
        .route("/invoices/{id}/void", post(routes::invoices::void_invoice))
        .route("/invoices/{id}/pay", post(routes::pay::pay_invoice))
        .route(
            "/webhook-endpoints",
            post(routes::webhooks::create_webhook_endpoint)
                .get(routes::webhooks::list_webhook_endpoints),
        )
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
        .with_state(state);

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/v1", api)
        .layer(TraceLayer::new_for_http())
}

pub async fn connect_pool(database_url: &str) -> anyhow::Result<sqlx::PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;
    Ok(pool)
}

pub fn make_state(pool: sqlx::PgPool, config: &Config) -> AppState {
    AppState {
        db: pool,
        psp: Arc::new(PspClient::new(config.psp_url.clone(), config.psp_timeout)),
        psp_reconcile: Arc::new(PspClient::new(
            config.psp_url.clone(),
            config.psp_reconcile_timeout,
        )),
    }
}

pub async fn spawn_workers(state: &AppState) {
    let pool = state.db.clone();
    let reconcile = (*state.psp_reconcile).clone();
    tokio::spawn(webhooks::run_webhook_worker(pool.clone()));
    tokio::spawn(workers::run_payment_reconciler(pool, reconcile));
}
