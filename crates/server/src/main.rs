mod admin;
mod api_access;
mod audit;
mod auth;
mod config;
mod database;
mod error;
mod library;
mod login_admission;
mod metrics;
mod password;
mod rooted_fs;
mod routes;
mod storage;
mod trusted_proxy;
mod upload_commit;

#[cfg(test)]
mod browser_session_tests;
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
    let state = AppState {
        pool,
        storage,
        config: config.clone(),
        login_admission: login_admission::LoginAdmission::default(),
    };
    admin::ensure_admin_user(&state).await?;
    let reconciliation = upload_commit::reconcile_all(&state).await?;
    info!(
        recovered = reconciliation.recovered,
        marked_unknown = reconciliation.marked_unknown,
        orphan_stages_removed = reconciliation.orphan_stages_removed,
        errors = reconciliation.errors,
        "upload commit reconciliation finished"
    );
    let app = routes::router(state).layer(TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    info!(address = %config.bind, "photo backup server listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
