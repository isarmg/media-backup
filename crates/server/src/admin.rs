use std::{
    net::SocketAddr,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{rejection::JsonRejection, ConnectInfo, Extension, Path, Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    middleware::Next,
    response::{Html, IntoResponse, Response},
    Json,
};
use sarmg_admin_auth::{
    is_token_shape, normalize_administrator_username, parse_cookie_value,
    require_administrator_same_origin, require_canonical_administrator_username,
    require_csrf_token_matches_hash, require_current_password_hash, require_single_csrf_token,
    AdministratorOriginMode, CSRF_HEADER, HOST_HEADER, ORIGIN_HEADER, SEC_FETCH_SITE_HEADER,
};
use sarmg_contracts::{AdministratorLoginRequest, AdministratorSession};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::{error::AppError, password, routes::AppState};

const SECURE_ADMIN_COOKIE: &str = "__Host-media_session";
const DEVELOPMENT_ADMIN_COOKIE: &str = "media_session";
const MAX_ADMIN_SESSIONS: i64 = 32;
const MAX_CSRF_TOKENS_PER_SESSION: i64 = 8;
const SESSION_TOUCH_INTERVAL_SECONDS: i64 = 60;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangeAdminPasswordRequest {
    current_password: String,
    new_password: String,
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
            "default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'",
        ),
    );
    response
}

pub(crate) async fn script() -> Response {
    static_asset_response(ADMIN_JS, "text/javascript; charset=utf-8")
}

pub(crate) async fn styles() -> Response {
    static_asset_response(ADMIN_CSS, "text/css; charset=utf-8")
}

fn static_asset_response(contents: &'static str, content_type: &'static str) -> Response {
    let mut response = contents.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

pub(crate) async fn ensure_admin_user(state: &AppState) -> Result<(), AppError> {
    let configured_username = state.config.admin_username.clone();
    let credentials = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, username, password_hash FROM auth_users ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await?;
    for (id, username, password_hash) in &credentials {
        require_canonical_administrator_username(username).map_err(|error| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("persisted administrator {id} has a non-canonical username: {error}"),
            )
        })?;
        require_current_password_hash(password_hash).map_err(|error| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("persisted administrator {id} has a non-current password hash: {error}"),
            )
        })?;
    }
    if credentials
        .iter()
        .any(|(_, username, _)| username == &configured_username)
    {
        return Ok(());
    }
    if !credentials.is_empty() {
        return Err(AppError::conflict(
            "configured administrator does not match the persisted administrator",
        ));
    }
    let password_hash =
        password::hash_current_password(state.config.admin_password.clone()).await?;
    let now = now_seconds()?;
    sqlx::query(
        "INSERT INTO auth_users(\
             id, username, password_hash, active, session_version, created_at, updated_at\
         ) VALUES (?, ?, ?, 1, 1, ?, ?)",
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
    uri: Uri,
    headers: HeaderMap,
    request: Result<Json<AdministratorLoginRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    reject_browser_bearer(&headers)?;
    verify_same_origin(&state, &headers, &uri)?;
    let Json(request) = request.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            AppError::new(StatusCode::PAYLOAD_TOO_LARGE, "request body is too large")
        } else {
            AppError::bad_request(error.to_string())
        }
    })?;
    let normalized_username = normalize_administrator_username(&request.username)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    sarmg_admin_auth::validate_password(&request.password)
        .map_err(|error| AppError::bad_request(error.to_string()))?;
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
         WHERE username = ? AND active = 1",
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

    let token = issue_random_token()?;
    let csrf_token = issue_random_token()?;
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
             id, user_id, token_hash, user_session_version, created_at, last_seen_at,\
             idle_expires_at, absolute_expires_at, user_agent, created_ip\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&session_id)
    .bind(&user.id)
    .bind(sarmg_admin_auth::token_hash(&token).to_vec())
    .bind(user.session_version)
    .bind(now)
    .bind(now)
    .bind(idle_expires_at)
    .bind(absolute_expires_at)
    .bind(user_agent)
    .bind(source.to_string())
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO admin_session_csrf_tokens(session_id, token_hash, created_at) VALUES (?, ?, ?)",
    )
    .bind(&session_id)
    .bind(sarmg_admin_auth::token_hash(&csrf_token).to_vec())
    .bind(now)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    let administrator_session = AdministratorSession::new(user.id, user.username, csrf_token)
        .map_err(|error| {
            tracing::error!(?error, "failed to construct administrator session contract");
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "administrator session contract error",
            )
        })?;
    let mut response = Json(administrator_session).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&state, &token))
            .map_err(|_| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "invalid session"))?,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private, max-age=0"),
    );
    Ok(response)
}

pub(crate) async fn logout(
    State(state): State<AppState>,
    Extension(identity): Extension<AdminIdentity>,
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
) -> Result<Response, AppError> {
    let csrf_token = issue_random_token()?;
    let now = now_seconds()?;
    let mut transaction = state.pool.begin().await?;
    let active: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM auth_sessions WHERE id = ? AND revoked_at IS NULL")
            .bind(&identity.session_id)
            .fetch_optional(&mut *transaction)
            .await?;
    if active.is_none() {
        return Err(AppError::unauthorized());
    }
    sqlx::query(
        "INSERT INTO admin_session_csrf_tokens(session_id, token_hash, created_at) VALUES (?, ?, ?)",
    )
    .bind(&identity.session_id)
    .bind(sarmg_admin_auth::token_hash(&csrf_token).to_vec())
    .bind(now)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "DELETE FROM admin_session_csrf_tokens WHERE session_id = ? AND id IN (\
             SELECT id FROM admin_session_csrf_tokens WHERE session_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?\
         )",
    )
    .bind(&identity.session_id)
    .bind(&identity.session_id)
    .bind(MAX_CSRF_TOKENS_PER_SESSION)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let administrator_session =
        AdministratorSession::new(identity.user_id, identity.username, csrf_token).map_err(
            |error| {
                tracing::error!(?error, "failed to construct administrator session contract");
                AppError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "administrator session contract error",
                )
            },
        )?;
    let mut response = Json(administrator_session).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private, max-age=0"),
    );
    Ok(response)
}

pub(crate) async fn change_admin_password(
    State(state): State<AppState>,
    Extension(identity): Extension<AdminIdentity>,
    Json(request): Json<ChangeAdminPasswordRequest>,
) -> Result<Response, AppError> {
    password::require_current_policy(&request.new_password)?;
    let current_hash: String =
        sqlx::query_scalar("SELECT password_hash FROM auth_users WHERE id = ?")
            .bind(&identity.user_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(AppError::unauthorized)?;
    if !password::verify_current_password(request.current_password, current_hash).await {
        return Err(AppError::unauthorized());
    }
    let new_hash = password::hash_current_password(request.new_password).await?;
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
        "SELECT s.id, s.user_id, u.username, s.absolute_expires_at \
         FROM auth_sessions s JOIN auth_users u ON u.id = s.user_id \
         WHERE s.token_hash = ? AND s.revoked_at IS NULL \
           AND s.idle_expires_at > ? AND s.absolute_expires_at > ? \
           AND u.active = 1 \
           AND u.session_version = s.user_session_version",
    )
    .bind(sarmg_admin_auth::token_hash(&provided).to_vec())
    .bind(now)
    .bind(now)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(AppError::unauthorized)?;
    let refreshed_idle = checked_expiry(now, state.config.admin_session_idle_seconds)?
        .min(stored.absolute_expires_at);
    if is_unsafe_method(request.method()) {
        verify_mutation_request(&state, request.headers(), request.uri(), &stored.id).await?;
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
               AND absolute_expires_at > ? AND last_seen_at <= ?",
        )
        .bind(now)
        .bind(refreshed_idle)
        .bind(session_id)
        .bind(now)
        .bind(now)
        .bind(now.saturating_sub(SESSION_TOUCH_INTERVAL_SECONDS))
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
    password::require_current_policy(&request.password)?;
    validate_policy(display_name, &storage_path, request.quota_bytes)?;
    ensure_unique_username(&state, username, None).await?;
    ensure_unique_path(&state, &storage_path, None).await?;
    state.storage.validate_account_path(&storage_path).await?;
    let password_hash = password::hash_current_password(request.password).await?;
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
    password::require_current_policy(&request.password)?;
    let password_hash = password::hash_current_password(request.password).await?;
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

async fn verify_mutation_request(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    session_id: &str,
) -> Result<(), AppError> {
    verify_same_origin(state, headers, uri)?;
    let csrf_name = HeaderName::from_static(CSRF_HEADER);
    let csrf_values = raw_header_values(headers, &csrf_name);
    require_single_csrf_token(&csrf_values)
        .map_err(|_| AppError::new(StatusCode::FORBIDDEN, "invalid CSRF token"))?;
    let candidates: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT token_hash FROM admin_session_csrf_tokens WHERE session_id = ? \
         ORDER BY created_at DESC, id DESC LIMIT ?",
    )
    .bind(session_id)
    .bind(MAX_CSRF_TOKENS_PER_SESSION)
    .fetch_all(&state.pool)
    .await?;
    if !candidates
        .iter()
        .any(|expected| require_csrf_token_matches_hash(&csrf_values, expected).is_ok())
    {
        return Err(AppError::new(StatusCode::FORBIDDEN, "invalid CSRF token"));
    }
    Ok(())
}

fn verify_same_origin(state: &AppState, headers: &HeaderMap, uri: &Uri) -> Result<(), AppError> {
    let mode = if state.config.development {
        AdministratorOriginMode::LoopbackDevelopmentHttp
    } else {
        AdministratorOriginMode::ProductionHttps
    };
    let origin_name = HeaderName::from_static(ORIGIN_HEADER);
    let host_name = HeaderName::from_static(HOST_HEADER);
    let site_name = HeaderName::from_static(SEC_FETCH_SITE_HEADER);
    let origins = raw_header_values(headers, &origin_name);
    let mut hosts = raw_header_values(headers, &host_name);
    if let Some(authority) = uri.authority() {
        hosts.push(authority.as_str().as_bytes());
    }
    let sites = raw_header_values(headers, &site_name);
    require_administrator_same_origin(mode, &origins, &hosts, &sites)
        .map(|_| ())
        .map_err(|_| AppError::new(StatusCode::FORBIDDEN, "invalid request origin"))
}

fn reject_browser_bearer(headers: &HeaderMap) -> Result<(), AppError> {
    if headers.contains_key(header::AUTHORIZATION) {
        return Err(AppError::unauthorized());
    }
    Ok(())
}

fn issue_random_token() -> Result<String, AppError> {
    sarmg_admin_auth::random_token().map_err(|error| {
        tracing::error!(?error, "administrator token generation failed");
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "administrator token generation failed",
        )
    })
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
    let cookie = single_cookie_header(headers)?;
    parse_cookie_value(cookie, name)
        .filter(|value| is_token_shape(value))
        .map(str::to_owned)
}

fn single_cookie_header(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::COOKIE).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}

fn raw_header_values<'headers>(
    headers: &'headers HeaderMap,
    name: &HeaderName,
) -> Vec<&'headers [u8]> {
    headers
        .get_all(name)
        .iter()
        .map(HeaderValue::as_bytes)
        .collect()
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
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private, max-age=0"),
    );
    response
}

// clients/web 是管理客户端的唯一源码位置；只嵌入经 Foundation 门禁和 Vite 生成的当前产物。
const ADMIN_HTML: &str = include_str!("../../../clients/web/dist/index.html");
const ADMIN_JS: &str = include_str!("../../../clients/web/dist/assets/admin.js");
const ADMIN_CSS: &str = include_str!("../../../clients/web/dist/assets/admin.css");

#[cfg(test)]
mod contract_tests {
    use super::{parse_cookie, ADMIN_HTML, ADMIN_JS};
    use axum::http::{header, HeaderMap, HeaderValue};

    #[test]
    fn embedded_browser_uses_only_the_current_admin_api_namespace() {
        assert!(ADMIN_HTML.contains("/admin/assets/admin.js"));
        assert!(ADMIN_HTML.contains("/admin/assets/admin.css"));
        assert!(ADMIN_HTML.contains("data-sarmg-scope"));
        for path in ["/api/v2/admin/overview", "/api/v2/admin/users"] {
            assert!(ADMIN_JS.contains(path), "missing browser API path {path}");
        }
        assert!(!ADMIN_JS.contains("\"/v2/admin"));
        assert!(!ADMIN_JS.contains("'/v2/admin"));
        assert!(!ADMIN_JS.contains("/admin/api"));
    }

    #[test]
    fn browser_cookie_requires_one_header_one_name_and_current_token_shape() {
        let token = sarmg_admin_auth::random_token().expect("generate canonical session token");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("other=x; media_session={token}"))
                .expect("valid test Cookie header"),
        );
        assert_eq!(parse_cookie(&headers, "media_session"), Some(token.clone()));

        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("media_session={token}; media_session={token}"))
                .expect("valid duplicate-cookie test header"),
        );
        assert_eq!(parse_cookie(&headers, "media_session"), None);

        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("media_session=short"),
        );
        assert_eq!(parse_cookie(&headers, "media_session"), None);

        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("media_session={token}"))
                .expect("valid first Cookie header"),
        );
        headers.append(
            header::COOKIE,
            HeaderValue::from_str(&format!("media_session={token}"))
                .expect("valid second Cookie header"),
        );
        assert_eq!(parse_cookie(&headers, "media_session"), None);
    }
}
