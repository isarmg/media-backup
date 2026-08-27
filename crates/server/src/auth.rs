use axum::{
    extract::{Request, State},
    http::header::AUTHORIZATION,
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{error::AppError, routes::AppState};

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub account_id: Uuid,
    pub device_id: Uuid,
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
    let record: Option<(Uuid, Uuid)> = sqlx::query_as(
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
    let (account_id, device_id) = record.ok_or_else(AppError::unauthorized)?;
    sqlx::query("UPDATE devices SET last_seen_at = now() WHERE id = $1")
        .bind(device_id)
        .execute(&state.pool)
        .await?;
    request.extensions_mut().insert(AuthContext {
        account_id,
        device_id,
    });
    Ok(next.run(request).await)
}
