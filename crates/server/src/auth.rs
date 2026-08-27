use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{error::AppError, routes::AppState};

pub async fn require_secure_transport(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    if !state.config.require_https || forwarded_as_https(request.headers()) {
        return Ok(next.run(request).await);
    }
    Err(AppError::new(
        StatusCode::UPGRADE_REQUIRED,
        "HTTPS is required",
    ))
}

fn forwarded_as_https(headers: &axum::http::HeaderMap) -> bool {
    if headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .next()
                .is_some_and(|value| value.trim() == "https")
        })
    {
        return true;
    }
    headers
        .get("forwarded")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .any(|entry| entry.trim().eq_ignore_ascii_case("proto=https"))
        })
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub account_id: Uuid,
    pub device_id: Uuid,
    pub actor_kind: String,
    pub actor_id: Uuid,
}

pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(AppError::unauthorized)?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or_else(AppError::unauthorized)?;
    let token_hash = Sha256::digest(token.as_bytes()).to_vec();
    let device: Option<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT d.account_id, d.id
        FROM devices d
        JOIN accounts a ON a.id = d.account_id
        WHERE d.token_hash = $1 AND a.enabled = TRUE
        "#,
    )
    .bind(token_hash)
    .fetch_optional(&state.pool)
    .await?;
    let (account_id, device_id, actor_kind, actor_id) =
        if let Some((account_id, device_id)) = device {
            sqlx::query("UPDATE devices SET last_seen_at = now() WHERE id = $1")
                .bind(device_id)
                .execute(&state.pool)
                .await?;
            (account_id, device_id, "device".to_owned(), device_id)
        } else {
            let api_key: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
                r#"
            SELECT k.account_id, k.device_id, k.id
            FROM api_keys k
            JOIN accounts a ON a.id = k.account_id
            WHERE k.token_hash = $1 AND k.revoked_at IS NULL AND a.enabled = TRUE
            "#,
            )
            .bind(Sha256::digest(token.as_bytes()).to_vec())
            .fetch_optional(&state.pool)
            .await?;
            let (account_id, device_id, api_key_id) = api_key.ok_or_else(AppError::unauthorized)?;
            sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
                .bind(api_key_id)
                .execute(&state.pool)
                .await?;
            (account_id, device_id, "api_key".to_owned(), api_key_id)
        };
    request.extensions_mut().insert(AuthContext {
        account_id,
        device_id,
        actor_kind,
        actor_id,
    });
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::forwarded_as_https;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn accepts_standard_https_proxy_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(forwarded_as_https(&headers));
        headers.remove("x-forwarded-proto");
        headers.insert(
            "forwarded",
            HeaderValue::from_static("for=192.0.2.1;proto=https"),
        );
        assert!(forwarded_as_https(&headers));
    }

    #[test]
    fn rejects_insecure_or_missing_proxy_headers() {
        let mut headers = HeaderMap::new();
        assert!(!forwarded_as_https(&headers));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));
        assert!(!forwarded_as_https(&headers));
    }
}
