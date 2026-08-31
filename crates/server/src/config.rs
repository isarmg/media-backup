use std::{env, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};

use crate::trusted_proxy::TrustedNetwork;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    pub admin_username: String,
    pub admin_password: String,
    pub max_part_bytes: usize,
    pub upload_global_concurrency: usize,
    pub upload_per_account_concurrency: usize,
    pub metrics_token: Option<String>,
    pub require_https: bool,
    pub development: bool,
    pub admin_session_idle_seconds: u64,
    pub admin_session_absolute_seconds: u64,
    pub trusted_proxy_cidrs: Vec<TrustedNetwork>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_url = env::var("DATABASE_URL").context("DATABASE_URL is required")?;
        let data_dir = PathBuf::from(env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_owned()));
        let bind: SocketAddr = env::var("BIND")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
            .parse()
            .context("BIND must be a socket address")?;
        let admin_username = configured_administrator_username(
            env::var("ADMIN_USERNAME").context("ADMIN_USERNAME is required")?,
        )?;
        let admin_password = env::var("ADMIN_PASSWORD").context("ADMIN_PASSWORD is required")?;
        sarmg_admin_auth::validate_password(&admin_password)
            .map_err(|error| anyhow::anyhow!("ADMIN_PASSWORD is invalid: {error}"))?;
        let max_part_bytes = env::var("MAX_PART_BYTES")
            .unwrap_or_else(|_| (64 * 1024 * 1024).to_string())
            .parse()
            .context("MAX_PART_BYTES must be an integer")?;
        let upload_global_concurrency = positive_usize("UPLOAD_GLOBAL_CONCURRENCY", 16)?;
        let upload_per_account_concurrency = positive_usize("UPLOAD_PER_ACCOUNT_CONCURRENCY", 4)?;
        if upload_per_account_concurrency > upload_global_concurrency {
            anyhow::bail!("UPLOAD_PER_ACCOUNT_CONCURRENCY cannot exceed UPLOAD_GLOBAL_CONCURRENCY");
        }
        let metrics_token = env::var("METRICS_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let require_https: bool = env::var("REQUIRE_HTTPS")
            .unwrap_or_else(|_| "true".to_owned())
            .parse()
            .context("REQUIRE_HTTPS must be true or false")?;
        let development: bool = env::var("DEVELOPMENT")
            .unwrap_or_else(|_| "false".to_owned())
            .parse()
            .context("DEVELOPMENT must be true or false")?;
        validate_security_mode(bind, require_https, development)?;
        let admin_session_idle_seconds: u64 = env::var("ADMIN_SESSION_IDLE_SECONDS")
            .unwrap_or_else(|_| "1800".to_owned())
            .parse()
            .context("ADMIN_SESSION_IDLE_SECONDS must be an integer")?;
        let admin_session_absolute_seconds: u64 = env::var("ADMIN_SESSION_ABSOLUTE_SECONDS")
            .unwrap_or_else(|_| "43200".to_owned())
            .parse()
            .context("ADMIN_SESSION_ABSOLUTE_SECONDS must be an integer")?;
        if admin_session_idle_seconds == 0
            || admin_session_absolute_seconds == 0
            || admin_session_idle_seconds > admin_session_absolute_seconds
        {
            anyhow::bail!("admin session TTLs must be non-zero and idle cannot exceed absolute");
        }
        let trusted_proxy_cidrs = env::var("TRUSTED_PROXY_CIDRS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::parse)
            .collect::<std::result::Result<Vec<_>, String>>()
            .map_err(anyhow::Error::msg)
            .context(
                "TRUSTED_PROXY_CIDRS must be a comma-separated list of exact IP/CIDR values",
            )?;
        Ok(Self {
            database_url,
            data_dir,
            bind,
            admin_username,
            admin_password,
            max_part_bytes,
            upload_global_concurrency,
            upload_per_account_concurrency,
            metrics_token,
            require_https,
            development,
            admin_session_idle_seconds,
            admin_session_absolute_seconds,
            trusted_proxy_cidrs,
        })
    }
}

fn configured_administrator_username(value: String) -> Result<String> {
    sarmg_admin_auth::normalize_administrator_username(&value)
        .map_err(|error| anyhow::anyhow!("ADMIN_USERNAME is invalid: {error}"))
}

fn positive_usize(name: &str, default: usize) -> Result<usize> {
    let value = env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .parse::<usize>()
        .with_context(|| format!("{name} must be an integer"))?;
    if value == 0 {
        anyhow::bail!("{name} must be non-zero");
    }
    Ok(value)
}

fn validate_security_mode(bind: SocketAddr, require_https: bool, development: bool) -> Result<()> {
    if development && !bind.ip().is_loopback() {
        anyhow::bail!("DEVELOPMENT=true requires a loopback BIND address");
    }
    if !development && !require_https {
        anyhow::bail!("production mode requires REQUIRE_HTTPS=true");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{configured_administrator_username, validate_security_mode};

    #[test]
    fn administrator_username_is_current_normalized_identity() {
        assert_eq!(
            configured_administrator_username(" Admin.Ops ".to_owned()).unwrap(),
            "admin.ops"
        );
        for invalid in ["ad", "admin@example.test", "-admin", "admin-", "管理员"] {
            assert!(configured_administrator_username(invalid.to_owned()).is_err());
        }
    }

    #[test]
    fn insecure_cookies_are_limited_to_explicit_loopback_development() {
        assert!(validate_security_mode("127.0.0.1:8080".parse().unwrap(), false, true).is_ok());
        assert!(validate_security_mode("[::1]:8080".parse().unwrap(), false, true).is_ok());
        assert!(validate_security_mode("0.0.0.0:8080".parse().unwrap(), false, true).is_err());
        assert!(validate_security_mode("127.0.0.1:8080".parse().unwrap(), false, false).is_err());
        assert!(validate_security_mode("0.0.0.0:8080".parse().unwrap(), true, false).is_ok());
    }
}
