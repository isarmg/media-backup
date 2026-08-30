mod admin;
mod api_access;
mod audit;
mod auth;
mod config;
mod database;
mod error;
mod library;
mod metrics;
mod password;
mod routes;
mod storage;

#[cfg(test)]
mod database_tests;

use anyhow::Result;
use config::Config;
use routes::AppState;
use storage::LocalStorage;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let config = Config::from_env()?;
    let pool = database::connect(&config.database_url).await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;
    let storage = LocalStorage::new(config.data_dir.clone()).await?;
    let app = routes::router(AppState {
        pool,
        storage,
        config: config.clone(),
    })
    .layer(TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    info!(address = %config.bind, "photo backup server listening");
    axum::serve(listener, app).await?;
    Ok(())
}
