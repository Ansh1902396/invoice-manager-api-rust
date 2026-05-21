use invoice_service::{build_router, config::Config, connect_pool, make_state, spawn_workers};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("invoice_service=info,tower_http=info,sqlx=warn")
        .init();

    let config = Config::from_env();
    let pool = connect_pool(&config.database_url).await?;
    info!("migrations applied");

    let state = make_state(pool, &config);
    spawn_workers(&state).await;

    let app = build_router(state);

    info!(addr = %config.listen_addr, "invoice service listening");
    info!("demo API key: dodo_live_demokey1234567890123456789012");

    let listener = tokio::net::TcpListener::bind(&config.listen_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
