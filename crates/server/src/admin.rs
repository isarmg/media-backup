use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{error::AppError, password, routes::AppState};

const ADMIN_COOKIE: &str = "photo_backup_admin";
const SESSION_SECONDS: u64 = 12 * 60 * 60;

#[derive(Debug, Deserialize)]
pub(crate) struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateUserRequest {
    username: String,
    password: String,
    display_name: String,
    storage_path: String,
    quota_bytes: i64,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateUserRequest {
    username: String,
    display_name: String,
    storage_path: String,
    quota_bytes: i64,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResetPasswordRequest {
    password: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AdminUser {
    id: Uuid,
    username: String,
    display_name: String,
    storage_path: String,
    quota_bytes: i64,
    used_bytes: i64,
    pending_bytes: i64,
    device_count: i64,
    resource_count: i64,
    enabled: bool,
    created_at: String,
    last_seen_at: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Overview {
    users: Vec<AdminUser>,
    total_users: i64,
    active_users: i64,
    unlimited_users: i64,
    used_bytes: i64,
    pending_bytes: i64,
    quota_bytes: i64,
}

pub(crate) async fn page() -> Response {
    let mut response = Html(ADMIN_HTML).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'",
        ),
    );
    response
}

pub(crate) async fn design_styles() -> Response {
    stylesheet_response(ADMIN_DESIGN_CSS, "public, max-age=3600")
}

pub(crate) async fn product_styles() -> Response {
    stylesheet_response(ADMIN_CSS, "no-cache")
}

fn stylesheet_response(contents: &'static str, cache_control: &'static str) -> Response {
    let mut response = contents.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}

pub(crate) async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Response, AppError> {
    let expected_username = state.config.admin_username.as_bytes();
    let provided_username = request.username.trim().as_bytes();
    let expected_password = state.config.admin_password.as_bytes();
    let provided_password = request.password.as_bytes();
    let username_matches = expected_username.len() == provided_username.len()
        && expected_username.ct_eq(provided_username).unwrap_u8() == 1;
    let password_matches = expected_password.len() == provided_password.len()
        && expected_password.ct_eq(provided_password).unwrap_u8() == 1;
    if !username_matches || !password_matches {
        return Err(AppError::unauthorized());
    }
    let cookie = format!(
        "{ADMIN_COOKIE}={}; Path=/admin; Max-Age={SESSION_SECONDS}; HttpOnly; SameSite=Strict",
        session_value(&state),
    );
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| AppError::bad_request("invalid session"))?,
    );
    Ok(response)
}

pub(crate) async fn logout() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "photo_backup_admin=; Path=/admin; Max-Age=0; HttpOnly; SameSite=Strict",
        ),
    );
    response
}

pub(crate) async fn require_admin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let provided = request
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                (name == ADMIN_COOKIE).then_some(value)
            })
        })
        .ok_or_else(AppError::unauthorized)?;
    let expected = session_value(&state);
    if expected.len() != provided.len()
        || expected.as_bytes().ct_eq(provided.as_bytes()).unwrap_u8() != 1
    {
        return Err(AppError::unauthorized());
    }
    Ok(next.run(request).await)
}

pub(crate) async fn overview(State(state): State<AppState>) -> Result<Json<Overview>, AppError> {
    let users = load_users(&state).await?;
    let total_users = users.len() as i64;
    let active_users = users.iter().filter(|user| user.enabled).count() as i64;
    let unlimited_users = users.iter().filter(|user| user.quota_bytes == 0).count() as i64;
    let used_bytes = users.iter().map(|user| user.used_bytes).sum();
    let pending_bytes = users.iter().map(|user| user.pending_bytes).sum();
    let quota_bytes = users
        .iter()
        .filter(|user| user.quota_bytes > 0)
        .map(|user| user.quota_bytes)
        .sum();
    Ok(Json(Overview {
        users,
        total_users,
        active_users,
        unlimited_users,
        used_bytes,
        pending_bytes,
        quota_bytes,
    }))
}

pub(crate) async fn create_user(
    State(state): State<AppState>,
    Json(request): Json<CreateUserRequest>,
) -> Result<Json<AdminUser>, AppError> {
    let id = Uuid::new_v4();
    let username = request.username.trim();
    let display_name = request.display_name.trim();
    let storage_path = if request.storage_path.trim().is_empty() {
        format!("blobs/{id}")
    } else {
        request.storage_path.trim().to_owned()
    };
    validate_username(username)?;
    password::validate_password(&request.password)?;
    validate_policy(display_name, &storage_path, request.quota_bytes)?;
    ensure_unique_username(&state, username, None).await?;
    ensure_unique_path(&state, &storage_path, None).await?;
    state.storage.validate_account_path(&storage_path).await?;
    let password_hash = password::hash_password(request.password).await?;
    sqlx::query(
        r#"
        INSERT INTO accounts(id, username, password_hash, display_name, storage_path, quota_bytes, enabled)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(username)
    .bind(password_hash)
    .bind(display_name)
    .bind(&storage_path)
    .bind(request.quota_bytes)
    .bind(request.enabled)
    .execute(&state.pool)
    .await?;
    Ok(Json(load_user(&state, id).await?))
}

pub(crate) async fn update_user(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateUserRequest>,
) -> Result<Json<AdminUser>, AppError> {
    let username = request.username.trim();
    let display_name = request.display_name.trim();
    let storage_path = request.storage_path.trim();
    validate_username(username)?;
    validate_policy(display_name, storage_path, request.quota_bytes)?;
    ensure_unique_username(&state, username, Some(id)).await?;
    ensure_unique_path(&state, storage_path, Some(id)).await?;
    state.storage.validate_account_path(storage_path).await?;
    let changed = sqlx::query(
        r#"
        UPDATE accounts
        SET username = $2, display_name = $3, storage_path = $4, quota_bytes = $5, enabled = $6
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(username)
    .bind(display_name)
    .bind(storage_path)
    .bind(request.quota_bytes)
    .bind(request.enabled)
    .execute(&state.pool)
    .await?;
    if changed.rows_affected() == 0 {
        return Err(AppError::not_found("user not found"));
    }
    Ok(Json(load_user(&state, id).await?))
}

pub(crate) async fn reset_user_password(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<StatusCode, AppError> {
    password::validate_password(&request.password)?;
    let password_hash = password::hash_password(request.password).await?;
    let changed = sqlx::query("UPDATE accounts SET password_hash = $2 WHERE id = $1")
        .bind(id)
        .bind(password_hash)
        .execute(&state.pool)
        .await?;
    if changed.rows_affected() == 0 {
        return Err(AppError::not_found("user not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn load_users(state: &AppState) -> Result<Vec<AdminUser>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT a.id, a.username, a.display_name, a.storage_path, a.quota_bytes, a.enabled,
               to_char(a.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at,
               COALESCE((SELECT SUM(b.stored_size) FROM blobs b WHERE b.account_id = a.id), 0)::BIGINT AS used_bytes,
               COALESCE((
                   SELECT SUM(p.expected_size)
                   FROM upload_parts p JOIN uploads u ON u.id = p.upload_id
                   WHERE u.account_id = a.id AND u.state = 'uploading'
               ), 0)::BIGINT AS pending_bytes,
               (SELECT COUNT(*) FROM devices d WHERE d.account_id = a.id)::BIGINT AS device_count,
               (SELECT COUNT(*) FROM resources r JOIN assets s ON s.id = r.asset_id WHERE s.account_id = a.id)::BIGINT AS resource_count,
               COALESCE(to_char((SELECT MAX(d.last_seen_at) FROM devices d WHERE d.account_id = a.id) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'), '') AS last_seen_at
        FROM accounts a
        ORDER BY a.created_at ASC
        "#,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.into_iter().map(row_to_user).collect())
}

async fn load_user(state: &AppState, id: Uuid) -> Result<AdminUser, AppError> {
    load_users(state)
        .await?
        .into_iter()
        .find(|user| user.id == id)
        .ok_or_else(|| AppError::not_found("user not found"))
}

fn row_to_user(row: sqlx::postgres::PgRow) -> AdminUser {
    AdminUser {
        id: row.get("id"),
        username: row.get("username"),
        display_name: row.get("display_name"),
        storage_path: row.get("storage_path"),
        quota_bytes: row.get("quota_bytes"),
        used_bytes: row.get("used_bytes"),
        pending_bytes: row.get("pending_bytes"),
        device_count: row.get("device_count"),
        resource_count: row.get("resource_count"),
        enabled: row.get("enabled"),
        created_at: row.get("created_at"),
        last_seen_at: row.get("last_seen_at"),
    }
}

fn validate_policy(
    display_name: &str,
    storage_path: &str,
    quota_bytes: i64,
) -> Result<(), AppError> {
    if display_name.is_empty() || display_name.len() > 100 {
        return Err(AppError::bad_request(
            "display_name must contain 1 to 100 characters",
        ));
    }
    if storage_path.is_empty()
        || storage_path.len() > 1024
        || storage_path.chars().any(char::is_control)
    {
        return Err(AppError::bad_request("invalid storage_path"));
    }
    if quota_bytes < 0 {
        return Err(AppError::bad_request("quota_bytes cannot be negative"));
    }
    Ok(())
}

fn validate_username(username: &str) -> Result<(), AppError> {
    if username.len() < 3
        || username.len() > 64
        || !username.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
    {
        return Err(AppError::bad_request(
            "username must contain 3 to 64 letters, digits, dots, dashes or underscores",
        ));
    }
    Ok(())
}

async fn ensure_unique_username(
    state: &AppState,
    username: &str,
    except_id: Option<Uuid>,
) -> Result<(), AppError> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE lower(username) = lower($1) AND ($2::UUID IS NULL OR id <> $2)",
    )
    .bind(username)
    .bind(except_id)
    .fetch_optional(&state.pool)
    .await?;
    if existing.is_some() {
        return Err(AppError::conflict("username is already in use"));
    }
    Ok(())
}

async fn ensure_unique_path(
    state: &AppState,
    storage_path: &str,
    except_id: Option<Uuid>,
) -> Result<(), AppError> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE storage_path = $1 AND ($2::UUID IS NULL OR id <> $2)",
    )
    .bind(storage_path)
    .bind(except_id)
    .fetch_optional(&state.pool)
    .await?;
    if existing.is_some() {
        return Err(AppError::conflict(
            "storage_path is already assigned to another user",
        ));
    }
    Ok(())
}

fn session_value(state: &AppState) -> String {
    let material = format!(
        "photo-backup-admin-session:{}:{}",
        state.config.admin_username, state.config.admin_password
    );
    URL_SAFE_NO_PAD.encode(Sha256::digest(material.as_bytes()))
}

const ADMIN_HTML: &str = include_str!("admin.html");
const ADMIN_CSS: &str = include_str!("admin.css");
const ADMIN_DESIGN_CSS: &str = include_str!("../../../vendor/sarmg-design/bundle.css");
