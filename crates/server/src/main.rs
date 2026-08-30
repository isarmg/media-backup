mod admin;
mod api_access;
mod audit;
mod auth;
mod backup;
mod config;
mod database;
mod error;
mod library;
mod login_admission;
mod metrics;
mod password;
mod rooted_fs;
mod routes;
mod runtime_lock;
mod storage;
mod trusted_proxy;
mod upload_commit;

#[cfg(test)]
mod browser_session_tests;
#[cfg(test)]
mod database_tests;

use std::path::PathBuf;

use anyhow::{Context, Result};
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
    let command = Command::parse(std::env::args().skip(1))?;
    let config = Config::from_env()?;
    match command {
        Command::BackupCreate(output) => {
            let summary = backup::create(&config, &output)?;
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "status": "backup-created",
                    "files": summary.files,
                    "bytes": summary.bytes,
                    "schema_version": summary.schema_version
                }))?
            );
            return Ok(());
        }
        Command::BackupVerify(input) => {
            let summary = backup::verify(&config, &input)?;
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "status": "backup-verified",
                    "files": summary.files,
                    "bytes": summary.bytes,
                    "schema_version": summary.schema_version
                }))?
            );
            return Ok(());
        }
        Command::BackupRestore(input) => {
            let summary = backup::restore(&config, &input)?;
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "status": "restored",
                    "files": summary.files,
                    "bytes": summary.bytes,
                    "schema_version": summary.schema_version
                }))?
            );
            return Ok(());
        }
        Command::Doctor => {
            println!("{}", serde_json::to_string(&backup::doctor(&config)?)?);
            return Ok(());
        }
        Command::Serve => {}
    }
    let _runtime_lock = runtime_lock::RuntimeLock::acquire(&config.database_url, &config.data_dir)?;
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

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve,
    BackupCreate(PathBuf),
    BackupVerify(PathBuf),
    BackupRestore(PathBuf),
    Doctor,
}

impl Command {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self> {
        let arguments: Vec<String> = arguments.collect();
        let parsed: Result<Self> = match arguments.as_slice() {
            [] => Ok(Self::Serve),
            [command] if command == "serve" => Ok(Self::Serve),
            [group, command, flag, path]
                if group == "backup" && command == "create" && flag == "--output" =>
            {
                Ok(Self::BackupCreate(PathBuf::from(path)))
            }
            [group, command, flag, path]
                if group == "backup" && command == "verify" && flag == "--input" =>
            {
                Ok(Self::BackupVerify(PathBuf::from(path)))
            }
            [group, command, flag, path]
                if group == "backup" && command == "restore" && flag == "--input" =>
            {
                Ok(Self::BackupRestore(PathBuf::from(path)))
            }
            [command, flag, path] if command == "restore" && flag == "--input" => {
                Ok(Self::BackupRestore(PathBuf::from(path)))
            }
            [command] if command == "doctor" => Ok(Self::Doctor),
            _ => anyhow::bail!(
                "usage: photo-backup-server [serve|backup create --output DIR|backup verify --input DIR|restore --input DIR|doctor]"
            ),
        };
        parsed.context("invalid command")
    }
}

#[cfg(test)]
mod command_tests {
    use super::Command;
    use std::path::PathBuf;

    fn parse(arguments: &[&str]) -> anyhow::Result<Command> {
        Command::parse(arguments.iter().map(|value| (*value).to_owned()))
    }

    #[test]
    fn maintenance_commands_require_explicit_input_or_output() {
        assert_eq!(parse(&[]).unwrap(), Command::Serve);
        assert_eq!(parse(&["serve"]).unwrap(), Command::Serve);
        assert_eq!(
            parse(&["backup", "create", "--output", "/tmp/new"]).unwrap(),
            Command::BackupCreate(PathBuf::from("/tmp/new"))
        );
        assert_eq!(
            parse(&["backup", "verify", "--input", "/tmp/snapshot"]).unwrap(),
            Command::BackupVerify(PathBuf::from("/tmp/snapshot"))
        );
        assert_eq!(
            parse(&["restore", "--input", "/tmp/snapshot"]).unwrap(),
            Command::BackupRestore(PathBuf::from("/tmp/snapshot"))
        );
        assert_eq!(parse(&["doctor"]).unwrap(), Command::Doctor);
        assert!(parse(&["backup", "create"]).is_err());
        assert!(parse(&["restore", "/tmp/snapshot"]).is_err());
    }
}
