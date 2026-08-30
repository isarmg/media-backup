use std::{
    net::SocketAddr,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{ConnectInfo, Extension, Path, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    middleware::Next,
    response::{Html, IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{error::AppError, password, routes::AppState};

const SECURE_ADMIN_COOKIE: &str = "__Host-photo_session";
const DEVELOPMENT_ADMIN_COOKIE: &str = "photo_session";
const MAX_ADMIN_SESSIONS: i64 = 32;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangeAdminPasswordRequest {
    current_password: String,
    new_password: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct SessionResponse {
    authenticated: bool,
    username: String,
    csrf_token: String,
}

#[derive(Debug, sqlx::FromRow)]
struct AuthUser {
    id: String,
    username: String,
    password_hash: String,
    session_version: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredSession {
    id: String,
    user_id: String,
    username: String,
    csrf_hash: Vec<u8>,
    absolute_expires_at: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct AdminIdentity {
    user_id: String,
    session_id: String,
    username: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateUserRequest {
    username: String,
    password: String,
    display_name: String,
    storage_path: String,
    quota_bytes: i64,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateUserRequest {
    username: String,
    display_name: String,
    storage_path: String,
    quota_bytes: i64,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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

pub(crate) async fn ensure_admin_user(state: &AppState) -> Result<(), AppError> {
    let configured_username = state.config.admin_username.trim().to_lowercase();
    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM auth_users WHERE lower(username) = ?")
            .bind(&configured_username)
            .fetch_optional(&state.pool)
            .await?;
    if existing.is_some() {
        return Ok(());
    }
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_users")
        .fetch_one(&state.pool)
        .await?;
    if user_count != 0 {
        return Err(AppError::conflict(
            "configured administrator does not match the persisted administrator",
        ));
    }
    let password_hash = password::hash_password(state.config.admin_password.clone()).await?;
    let now = now_seconds()?;
    sqlx::query(
        "INSERT INTO auth_users(\
             id, username, password_hash, role, active, session_version, created_at, updated_at\
         ) VALUES (?, ?, ?, 'admin', 1, 1, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(configured_username)
    .bind(password_hash)
    .bind(now)
    .bind(now)
    .execute(&state.pool)
    .await?;
    Ok(())
}

pub(crate) async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Response, AppError> {
    reject_browser_bearer(&headers)?;
    verify_same_origin(&headers)?;
    let normalized_username = request.username.trim().to_lowercase();
    let source = crate::trusted_proxy::resolve_client_ip(
        peer.ip(),
        &headers,
        &state.config.trusted_proxy_cidrs,
    )?;
    state.login_admission.check_source(source)?;
    state
        .login_admission
        .check_account(&format!("admin:{normalized_username}"))?;
    let user = sqlx::query_as::<_, AuthUser>(
        "SELECT id, username, password_hash, session_version FROM auth_users \
         WHERE lower(username) = ? AND active = 1 AND role = 'admin'",
    )
    .bind(normalized_username)
    .fetch_optional(&state.pool)
    .await?;
    let verified = state
        .login_admission
        .verify(
            request.password,
            user.as_ref().map(|user| user.password_hash.clone()),
        )
        .await?;
    let Some(user) = user.filter(|_| verified) else {
        return Err(AppError::unauthorized());
    };

    let token = random_token();
    let csrf_token = random_token();
    let now = now_seconds()?;
    let idle_expires_at = checked_expiry(now, state.config.admin_session_idle_seconds)?;
    let absolute_expires_at = checked_expiry(now, state.config.admin_session_absolute_seconds)?;
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(512).collect::<String>());
    let session_id = Uuid::new_v4().to_string();
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        "DELETE FROM auth_sessions \
         WHERE revoked_at IS NOT NULL OR idle_expires_at <= ? OR absolute_expires_at <= ?",
    )
    .bind(now)
    .bind(now)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "DELETE FROM auth_sessions WHERE user_id = ? AND id IN (\
             SELECT id FROM auth_sessions WHERE user_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?\
         )",
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(MAX_ADMIN_SESSIONS - 1)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO auth_sessions(\
             id, user_id, token_hash, csrf_hash, user_session_version, created_at, last_seen_at,\
             idle_expires_at, absolute_expires_at, user_agent, created_ip\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(session_id)
    .bind(&user.id)
    .bind(token_hash(&token))
    .bind(token_hash(&csrf_token))
    .bind(user.session_version)
    .bind(now)
    .bind(now)
    .bind(idle_expires_at)
    .bind(absolute_expires_at)
    .bind(user_agent)
    .bind(source.to_string())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    let mut response = Json(SessionResponse {
        authenticated: true,
        username: user.username,
        csrf_token,
    })
    .into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&state, &token))
            .map_err(|_| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "invalid session"))?,
    );
    Ok(response)
}

pub(crate) async fn logout(
    State(state): State<AppState>,
    Extension(identity): Extension<AdminIdentity>,
    Json(_request): Json<photo_backup_protocol::EmptyRequest>,
) -> Result<Response, AppError> {
    sqlx::query("UPDATE auth_sessions SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(now_seconds()?)
        .bind(identity.session_id)
        .execute(&state.pool)
        .await?;
    Ok(expire_session_response(&state, StatusCode::NO_CONTENT))
}

pub(crate) async fn session(
    State(state): State<AppState>,
    Extension(identity): Extension<AdminIdentity>,
) -> Result<Json<SessionResponse>, AppError> {
    let csrf_token = random_token();
    let changed =
        sqlx::query("UPDATE auth_sessions SET csrf_hash = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(token_hash(&csrf_token))
            .bind(&identity.session_id)
            .execute(&state.pool)
            .await?;
    if changed.rows_affected() != 1 {
        return Err(AppError::unauthorized());
    }
    Ok(Json(SessionResponse {
        authenticated: true,
        username: identity.username,
        csrf_token,
    }))
}

pub(crate) async fn change_admin_password(
    State(state): State<AppState>,
    Extension(identity): Extension<AdminIdentity>,
    Json(request): Json<ChangeAdminPasswordRequest>,
) -> Result<Response, AppError> {
    password::validate_password(&request.new_password)?;
    let current_hash: String =
        sqlx::query_scalar("SELECT password_hash FROM auth_users WHERE id = ?")
            .bind(&identity.user_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(AppError::unauthorized)?;
    if !password::verify_password(request.current_password, current_hash).await {
        return Err(AppError::unauthorized());
    }
    let new_hash = password::hash_password(request.new_password).await?;
    let changed = sqlx::query("UPDATE auth_users SET password_hash = ? WHERE id = ?")
        .bind(new_hash)
        .bind(identity.user_id)
        .execute(&state.pool)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(AppError::unauthorized());
    }
    Ok(expire_session_response(&state, StatusCode::NO_CONTENT))
}

pub(crate) async fn require_admin(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    reject_browser_bearer(request.headers())?;
    let provided = parse_cookie(request.headers(), session_cookie_name(&state))
        .ok_or_else(AppError::unauthorized)?;
    let now = now_seconds()?;
    let stored = sqlx::query_as::<_, StoredSession>(
        "SELECT s.id, s.user_id, u.username, s.csrf_hash, s.absolute_expires_at \
         FROM auth_sessions s JOIN auth_users u ON u.id = s.user_id \
         WHERE s.token_hash = ? AND s.revoked_at IS NULL \
           AND s.idle_expires_at > ? AND s.absolute_expires_at > ? \
           AND u.active = 1 AND u.role = 'admin' \
           AND u.session_version = s.user_session_version",
    )
    .bind(token_hash(&provided))
    .bind(now)
    .bind(now)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(AppError::unauthorized)?;
    let refreshed_idle = checked_expiry(now, state.config.admin_session_idle_seconds)?
        .min(stored.absolute_expires_at);
    if is_unsafe_method(request.method()) {
        verify_mutation_request(request.headers(), &stored.csrf_hash)?;
    }
    let session_id = stored.id.clone();
    request.extensions_mut().insert(AdminIdentity {
        user_id: stored.user_id,
        session_id: stored.id,
        username: stored.username,
    });
    let response = next.run(request).await;
    if response.status().is_success() {
        if let Err(error) = sqlx::query(
            "UPDATE auth_sessions SET last_seen_at = ?, idle_expires_at = ? \
             WHERE id = ? AND revoked_at IS NULL AND idle_expires_at > ? \
               AND absolute_expires_at > ?",
        )
        .bind(now)
        .bind(refreshed_idle)
        .bind(session_id)
        .bind(now)
        .bind(now)
        .execute(&state.pool)
        .await
        {
            tracing::warn!(?error, "failed to refresh successful admin session use");
        }
    }
    Ok(response)
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
        INSERT INTO accounts(
            id, username, password_hash, display_name, storage_path, quota_bytes, enabled, created_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'))
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
        SET username = ?, display_name = ?, storage_path = ?, quota_bytes = ?, enabled = ?
        WHERE id = ?
        "#,
    )
    .bind(username)
    .bind(display_name)
    .bind(storage_path)
    .bind(request.quota_bytes)
    .bind(request.enabled)
    .bind(id)
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
    let changed = sqlx::query("UPDATE accounts SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(id)
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
               strftime('%Y-%m-%dT%H:%M:%SZ', a.created_at) AS created_at,
               COALESCE((SELECT SUM(b.stored_size) FROM blobs b WHERE b.account_id = a.id), 0) AS used_bytes,
               COALESCE((
                   SELECT SUM(p.expected_size)
                   FROM upload_parts p JOIN uploads u ON u.id = p.upload_id
                   WHERE u.account_id = a.id AND u.state = 'uploading'
               ), 0) AS pending_bytes,
               (SELECT COUNT(*) FROM devices d WHERE d.account_id = a.id) AS device_count,
               (SELECT COUNT(*) FROM resources r JOIN assets s ON s.id = r.asset_id WHERE s.account_id = a.id) AS resource_count,
               COALESCE(strftime('%Y-%m-%dT%H:%M:%SZ', (SELECT MAX(d.last_seen_at) FROM devices d WHERE d.account_id = a.id)), '') AS last_seen_at
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

fn row_to_user(row: sqlx::sqlite::SqliteRow) -> AdminUser {
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
        "SELECT id FROM accounts \
         WHERE lower(username) = lower(?1) AND (?2 IS NULL OR id <> ?2)",
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
    let existing = sqlx::query("SELECT storage_path FROM accounts WHERE ?1 IS NULL OR id <> ?1")
        .bind(except_id)
        .fetch_all(&state.pool)
        .await?;
    for row in existing {
        let assigned: String = row.get("storage_path");
        if state
            .storage
            .account_paths_overlap(storage_path, &assigned)?
        {
            return Err(AppError::conflict(
                "storage_path overlaps another user's directory",
            ));
        }
    }
    Ok(())
}

fn is_unsafe_method(method: &Method) -> bool {
    !matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

fn verify_mutation_request(headers: &HeaderMap, expected_csrf_hash: &[u8]) -> Result<(), AppError> {
    verify_same_origin(headers)?;
    let provided = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::new(StatusCode::FORBIDDEN, "invalid CSRF token"))?;
    let provided_hash = token_hash(provided);
    if provided_hash.len() != expected_csrf_hash.len()
        || provided_hash.ct_eq(expected_csrf_hash).unwrap_u8() != 1
    {
        return Err(AppError::new(StatusCode::FORBIDDEN, "invalid CSRF token"));
    }
    Ok(())
}

fn verify_same_origin(headers: &HeaderMap) -> Result<(), AppError> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Uri>().ok())
        .filter(|uri| matches!(uri.scheme_str(), Some("http" | "https")))
        .ok_or_else(|| AppError::new(StatusCode::FORBIDDEN, "invalid request origin"))?;
    if origin
        .path_and_query()
        .is_some_and(|value| value.as_str() != "/")
    {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "invalid request origin",
        ));
    }
    let origin_authority = origin
        .authority()
        .ok_or_else(|| AppError::new(StatusCode::FORBIDDEN, "invalid request origin"))?;
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
        .ok_or_else(|| AppError::new(StatusCode::FORBIDDEN, "invalid request host"))?;
    let default_port = if origin.scheme_str() == Some("https") {
        443
    } else {
        80
    };
    if !origin_authority.host().eq_ignore_ascii_case(host.host())
        || origin_authority.port_u16().unwrap_or(default_port)
            != host.port_u16().unwrap_or(default_port)
    {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "cross-origin request rejected",
        ));
    }
    Ok(())
}

fn reject_browser_bearer(headers: &HeaderMap) -> Result<(), AppError> {
    if headers.contains_key(header::AUTHORIZATION) {
        return Err(AppError::unauthorized());
    }
    Ok(())
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn now_seconds() -> Result<i64, AppError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "system clock error"))?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|_| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "system clock error"))
}

fn checked_expiry(now: i64, ttl_seconds: u64) -> Result<i64, AppError> {
    let ttl = i64::try_from(ttl_seconds)
        .map_err(|_| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "invalid session TTL"))?;
    now.checked_add(ttl)
        .ok_or_else(|| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "invalid session TTL"))
}

fn session_cookie_name(state: &AppState) -> &'static str {
    if state.config.development {
        DEVELOPMENT_ADMIN_COOKIE
    } else {
        SECURE_ADMIN_COOKIE
    }
}

fn session_cookie(state: &AppState, token: &str) -> String {
    let mut cookie = format!(
        "{}={token}; Path=/; Max-Age={}; HttpOnly; SameSite=Strict",
        session_cookie_name(state),
        state.config.admin_session_absolute_seconds
    );
    if !state.config.development {
        cookie.push_str("; Secure");
    }
    cookie
}

fn parse_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == name && !value.is_empty()).then(|| value.to_owned())
        })
}

fn expire_session_response(state: &AppState, status: StatusCode) -> Response {
    let mut cookie = format!(
        "{}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict",
        session_cookie_name(state)
    );
    if !state.config.development {
        cookie.push_str("; Secure");
    }
    let mut response = status.into_response();
    if let Ok(cookie) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

const ADMIN_HTML: &str = include_str!("admin.html");
const ADMIN_CSS: &str = include_str!("admin.css");
const ADMIN_DESIGN_CSS: &str = include_str!("../../../vendor/sarmg-design/bundle.css");

#[cfg(test)]
mod contract_tests {
    use super::ADMIN_HTML;

    #[test]
    fn embedded_browser_uses_only_the_current_admin_api_namespace() {
        for path in [
            "/v2/admin/login",
            "/v2/admin/session",
            "/v2/admin/overview",
            "/v2/admin/users",
            "/v2/admin/logout",
        ] {
            assert!(ADMIN_HTML.contains(path), "missing browser API path {path}");
        }
        assert!(!ADMIN_HTML.contains("/admin/api"));
        assert!(!ADMIN_HTML.contains("/v1/"));
    }
}
