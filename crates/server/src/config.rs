use std::{env, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    pub admin_username: String,
    pub admin_password: String,
    pub max_part_bytes: usize,
    pub metrics_token: Option<String>,
    pub require_https: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_url = env::var("DATABASE_URL").context("DATABASE_URL is required")?;
        let data_dir = PathBuf::from(env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_owned()));
        let bind = env::var("BIND")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
            .parse()
            .context("BIND must be a socket address")?;
        let admin_username = env::var("ADMIN_USERNAME").context("ADMIN_USERNAME is required")?;
        if admin_username.trim().is_empty() || admin_username.len() > 100 {
            anyhow::bail!("ADMIN_USERNAME must contain 1 to 100 characters");
        }
        let admin_password = env::var("ADMIN_PASSWORD").context("ADMIN_PASSWORD is required")?;
        if admin_password.len() < 12 {
            anyhow::bail!("ADMIN_PASSWORD must contain at least 12 characters");
        }
        let max_part_bytes = env::var("MAX_PART_BYTES")
            .unwrap_or_else(|_| (64 * 1024 * 1024).to_string())
            .parse()
            .context("MAX_PART_BYTES must be an integer")?;
        let metrics_token = env::var("METRICS_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let require_https = env::var("REQUIRE_HTTPS")
            .unwrap_or_else(|_| "true".to_owned())
            .parse()
            .context("REQUIRE_HTTPS must be true or false")?;
        Ok(Self {
            database_url,
            data_dir,
            bind,
            admin_username,
            admin_password,
            max_part_bytes,
            metrics_token,
            require_https,
        })
    }
}
