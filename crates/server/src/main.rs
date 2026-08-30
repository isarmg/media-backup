mod admin;
mod api_access;
mod audit;
mod auth;
mod config;
mod database;
mod doctor;
mod error;
mod library;
mod login_admission;
mod metrics;
mod password;
mod release;
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

use anyhow::{Context, Result};
use config::Config;
use routes::AppState;
use std::path::PathBuf;
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
    match &command {
        Command::ReleaseIdentity => {
            println!("{}", release::identity_json()?);
            return Ok(());
        }
        Command::ReleaseVerify(root) => {
            let identity = release::verify(root)?;
            println!("{}", release::verification_line(&identity));
            return Ok(());
        }
        Command::ReleaseVerifyInstalled(root) => {
            let identity = release::verify_installed(root)?;
            println!("{}", release::verification_line(&identity));
            return Ok(());
        }
        Command::ServeRelease(root) => {
            release::verify_runtime(root)?;
        }
        Command::Serve => release::ensure_unbound_development_serve()?,
        Command::Doctor => {}
    }
    let config = Config::from_env()?;
    match command {
        Command::Doctor => {
            println!("{}", serde_json::to_string(&doctor::run(&config)?)?);
            return Ok(());
        }
        Command::Serve | Command::ServeRelease(_) => {}
        Command::ReleaseIdentity
        | Command::ReleaseVerify(_)
        | Command::ReleaseVerifyInstalled(_) => {
            unreachable!("release commands return before configuration is loaded")
        }
    }
    let _runtime_lock = runtime_lock::RuntimeLock::acquire(&config.database_url, &config.data_dir)?;
    let pool = database::connect(&config.database_url).await?;
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
    ServeRelease(PathBuf),
    Doctor,
    ReleaseIdentity,
    ReleaseVerify(PathBuf),
    ReleaseVerifyInstalled(PathBuf),
}

impl Command {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self> {
        let arguments: Vec<String> = arguments.collect();
        let parsed: Result<Self> = match arguments.as_slice() {
            [] => Ok(Self::Serve),
            [command] if command == "serve" => Ok(Self::Serve),
            [command, root] if command == "serve-release" => {
                Ok(Self::ServeRelease(PathBuf::from(root)))
            }
            [command] if command == "doctor" => Ok(Self::Doctor),
            [command] if command == "release-identity" => Ok(Self::ReleaseIdentity),
            [command, root] if command == "release-verify" => {
                Ok(Self::ReleaseVerify(PathBuf::from(root)))
            }
            [command, root] if command == "release-verify-installed" => {
                Ok(Self::ReleaseVerifyInstalled(PathBuf::from(root)))
            }
            _ => anyhow::bail!(
                "usage: photo-backup-server [serve|serve-release RELEASE_ROOT|doctor|release-identity|release-verify RELEASE_ROOT|release-verify-installed RELEASE_ROOT]"
            ),
        };
        parsed.context("invalid command")
    }
}

#[cfg(test)]
mod command_tests {
    use std::path::PathBuf;

    use super::Command;

    fn parse(arguments: &[&str]) -> anyhow::Result<Command> {
        Command::parse(arguments.iter().map(|value| (*value).to_owned()))
    }

    #[test]
    fn only_current_product_commands_are_accepted() {
        assert_eq!(parse(&[]).unwrap(), Command::Serve);
        assert_eq!(parse(&["serve"]).unwrap(), Command::Serve);
        assert_eq!(
            parse(&["serve-release", "/opt/isarmg/photo-backup/releases/0.2.0"]).unwrap(),
            Command::ServeRelease(PathBuf::from("/opt/isarmg/photo-backup/releases/0.2.0"))
        );
        assert_eq!(parse(&["doctor"]).unwrap(), Command::Doctor);
        assert_eq!(
            parse(&["release-identity"]).unwrap(),
            Command::ReleaseIdentity
        );
        assert_eq!(
            parse(&["release-verify", "/opt/isarmg/photo-backup/releases/0.2.0"]).unwrap(),
            Command::ReleaseVerify(PathBuf::from("/opt/isarmg/photo-backup/releases/0.2.0"))
        );
        assert_eq!(
            parse(&[
                "release-verify-installed",
                "/opt/isarmg/photo-backup/releases/0.2.0"
            ])
            .unwrap(),
            Command::ReleaseVerifyInstalled(PathBuf::from(
                "/opt/isarmg/photo-backup/releases/0.2.0"
            ))
        );
        assert!(parse(&["backup", "create"]).is_err());
        assert!(parse(&["backup", "create", "--output", "/tmp/new"]).is_err());
        assert!(parse(&["restore", "--input", "/tmp/snapshot"]).is_err());
    }
}
