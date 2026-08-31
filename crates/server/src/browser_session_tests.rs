use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Method, Request, Response, StatusCode},
    Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    admin,
    config::Config,
    database,
    routes::{router, AppState},
    storage::LocalStorage,
};

const ADMIN_USERNAME: &str = "persisted-admin";
const ADMIN_PASSWORD: &str = "persisted-admin-password";
const NEW_ADMIN_PASSWORD: &str = "new-persisted-admin-password";

struct TestWorkspace(PathBuf);

impl TestWorkspace {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("media-browser-auth-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create browser auth test directory");
        Self(root)
    }

    fn database(&self) -> PathBuf {
        self.0.join("media.db")
    }

    fn data(&self) -> PathBuf {
        self.0.join("data")
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn database_url(path: &Path) -> String {
    format!("sqlite://{}", path.display())
}

async fn test_state(workspace: &TestWorkspace) -> (AppState, SqlitePool) {
    let database_url = database_url(&workspace.database());
    let pool = database::connect(&database_url)
        .await
        .expect("connect browser auth SQLite");
    let state = AppState {
        pool: pool.clone(),
        storage: LocalStorage::new(workspace.data())
            .await
            .expect("create browser auth storage"),
        config: Config {
            database_url,
            data_dir: workspace.data(),
            bind: "127.0.0.1:0".parse().unwrap(),
            admin_username: ADMIN_USERNAME.to_owned(),
            admin_password: ADMIN_PASSWORD.to_owned(),
            max_part_bytes: 1024 * 1024,
            upload_global_concurrency: 16,
            upload_per_account_concurrency: 4,
            metrics_token: None,
            require_https: false,
            development: true,
            admin_session_idle_seconds: 1_800,
            admin_session_absolute_seconds: 43_200,
            trusted_proxy_cidrs: Vec::new(),
        },
        login_admission: crate::login_admission::LoginAdmission::default(),
        upload_admission: crate::routes::UploadAdmission::new(16, 4),
    };
    admin::ensure_admin_user(&state)
        .await
        .expect("seed persisted administrator");
    (state, pool)
}

#[tokio::test]
async fn startup_rejects_noncanonical_admin_identity_and_noncurrent_password_hash() {
    let workspace = TestWorkspace::new();
    let (state, pool) = test_state(&workspace).await;
    let current_hash: String = sqlx::query_scalar("SELECT password_hash FROM auth_users")
        .fetch_one(&pool)
        .await
        .expect("load current administrator hash");

    sqlx::query("UPDATE auth_users SET password_hash = 'malformed-password-hash'")
        .execute(&pool)
        .await
        .expect("install invalid test hash");
    let error = admin::ensure_admin_user(&state)
        .await
        .expect_err("non-current administrator hash must fail startup");
    assert!(error.to_string().contains("non-current password hash"));

    let mut connection = pool
        .acquire()
        .await
        .expect("acquire connection for corruption fixture");
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *connection)
        .await
        .expect("disable CHECK constraints for corruption fixture");
    sqlx::query(
        "UPDATE auth_users SET password_hash = ?, username = 'persisted-admin@example.test'",
    )
    .bind(current_hash)
    .execute(&mut *connection)
    .await
    .expect("install non-canonical test username");
    sqlx::query("PRAGMA ignore_check_constraints = OFF")
        .execute(&mut *connection)
        .await
        .expect("restore CHECK constraints after corruption fixture");
    drop(connection);
    let error = admin::ensure_admin_user(&state)
        .await
        .expect_err("non-canonical administrator username must fail startup");
    assert!(error.to_string().contains("non-canonical username"));

    drop(pool);
    state.pool.close().await;
}

fn browser_request(
    method: Method,
    path: &str,
    body: Value,
    cookie: Option<&str>,
    csrf: Option<&str>,
    origin: &str,
) -> Request<Body> {
    browser_request_from(
        "127.0.0.1:42000".parse().unwrap(),
        method,
        path,
        body,
        cookie,
        csrf,
        origin,
    )
}

#[allow(clippy::too_many_arguments)]
fn browser_request_from(
    peer: std::net::SocketAddr,
    method: Method,
    path: &str,
    body: Value,
    cookie: Option<&str>,
    csrf: Option<&str>,
    origin: &str,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::HOST, "localhost")
        .header(header::ORIGIN, origin)
        .header("sec-fetch-site", "same-origin");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    let mut request = builder.body(Body::from(body.to_string())).unwrap();
    request.extensions_mut().insert(ConnectInfo(peer));
    request
}

async fn respond(app: &Router, request: Request<Body>) -> Response<Body> {
    app.clone().oneshot(request).await.expect("router response")
}

async fn login(app: &Router, password: &str) -> (String, String) {
    let response = respond(
        app,
        browser_request(
            Method::POST,
            "/api/v2/auth/login",
            json!({"username": ADMIN_USERNAME, "password": password}),
            None,
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "no-store, private, max-age=0"
    );
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("read login body"),
    )
    .expect("decode login body");
    let object = body.as_object().expect("administrator session object");
    assert_eq!(object.len(), 5, "administrator session must be exact");
    assert_eq!(body["authenticated"], true);
    Uuid::parse_str(body["user_id"].as_str().expect("administrator user id"))
        .expect("administrator user id UUID");
    assert_eq!(body["username"], ADMIN_USERNAME);
    assert_eq!(body["role"], "admin");
    (cookie, body["csrf_token"].as_str().unwrap().to_owned())
}

fn cookie_token(cookie: &str) -> &str {
    cookie.split_once('=').unwrap().1
}

#[tokio::test]
async fn login_rejects_policy_invalid_credentials_before_account_lookup() {
    let workspace = TestWorkspace::new();
    let (state, pool) = test_state(&workspace).await;
    let app = router(state);
    for body in [
        json!({"email": "admin@example.test", "password": "valid-password"}),
        json!({"username": "admin@example.test", "password": "valid-password"}),
        json!({"username": ADMIN_USERNAME, "password": "short"}),
        json!({"username": ADMIN_USERNAME, "password": ""}),
    ] {
        let response = respond(
            &app,
            browser_request(
                Method::POST,
                "/api/v2/auth/login",
                body,
                None,
                None,
                "http://localhost",
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let wrong_but_valid = respond(
        &app,
        browser_request(
            Method::POST,
            "/api/v2/auth/login",
            json!({"username": "unknown", "password": "wrong-password"}),
            None,
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(wrong_but_valid.status(), StatusCode::UNAUTHORIZED);
    pool.close().await;
}

#[tokio::test]
async fn sessions_are_random_revocable_and_bound_to_origin_and_csrf() {
    let workspace = TestWorkspace::new();
    let (state, pool) = test_state(&workspace).await;
    let app = router(state);
    let (first_cookie, first_csrf) = login(&app, ADMIN_PASSWORD).await;
    let (second_cookie, second_csrf) = login(&app, ADMIN_PASSWORD).await;
    assert_ne!(first_cookie, second_cookie);
    assert_ne!(first_csrf, second_csrf);

    let hashes: Vec<Vec<u8>> = sqlx::query_scalar("SELECT token_hash FROM auth_sessions")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(hashes.len(), 2);
    assert!(hashes.iter().all(|hash| hash.len() == 32));
    assert!(hashes.contains(&Sha256::digest(cookie_token(&first_cookie)).to_vec()));
    assert!(!hashes
        .iter()
        .any(|hash| hash.as_slice() == cookie_token(&first_cookie).as_bytes()));

    let missing_csrf = respond(
        &app,
        browser_request(
            Method::POST,
            "/api/v2/auth/logout",
            json!({}),
            Some(&first_cookie),
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let cross_session = respond(
        &app,
        browser_request(
            Method::POST,
            "/api/v2/auth/logout",
            json!({}),
            Some(&first_cookie),
            Some(&second_csrf),
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(cross_session.status(), StatusCode::FORBIDDEN);

    let cross_origin = respond(
        &app,
        browser_request(
            Method::POST,
            "/api/v2/auth/logout",
            json!({}),
            Some(&first_cookie),
            Some(&first_csrf),
            "http://attacker.test",
        ),
    )
    .await;
    assert_eq!(cross_origin.status(), StatusCode::FORBIDDEN);

    let mut ambiguous = browser_request(
        Method::GET,
        "/api/v2/admin/overview",
        json!({}),
        Some(&first_cookie),
        None,
        "http://localhost",
    );
    ambiguous.headers_mut().insert(
        header::AUTHORIZATION,
        "Bearer device-token".parse().unwrap(),
    );
    assert_eq!(
        respond(&app, ambiguous).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let machine_with_cookie = Request::builder()
        .uri("/v2/timeline")
        .header(header::COOKIE, &first_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        respond(&app, machine_with_cookie).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let logout = respond(
        &app,
        browser_request(
            Method::POST,
            "/api/v2/auth/logout",
            json!({}),
            Some(&first_cookie),
            Some(&first_csrf),
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let revoked = respond(
        &app,
        browser_request(
            Method::GET,
            "/api/v2/admin/overview",
            json!({}),
            Some(&first_cookie),
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

    let session = respond(
        &app,
        browser_request(
            Method::GET,
            "/api/v2/auth/session",
            json!({}),
            Some(&second_cookie),
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(session.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(session.into_body(), 16 * 1024).await.unwrap()).unwrap();
    assert_eq!(body.as_object().unwrap().len(), 5);
    assert_eq!(body["authenticated"], true);
    assert_eq!(body["username"], ADMIN_USERNAME);
    assert_eq!(body["role"], "admin");
    Uuid::parse_str(body["user_id"].as_str().unwrap()).unwrap();
    let rotated_csrf = body["csrf_token"].as_str().unwrap();
    assert_ne!(rotated_csrf, second_csrf);
    let previous_tab_csrf = respond(
        &app,
        browser_request(
            Method::POST,
            "/api/v2/auth/logout",
            json!({}),
            Some(&second_cookie),
            Some(&second_csrf),
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(previous_tab_csrf.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn sessions_survive_restart_and_security_changes_invalidate_all_sessions() {
    let workspace = TestWorkspace::new();
    let (state, pool) = test_state(&workspace).await;
    let app = router(state);
    let (cookie, csrf) = login(&app, ADMIN_PASSWORD).await;
    drop(app);
    pool.close().await;

    let (restarted, restarted_pool) = test_state(&workspace).await;
    let restarted_app = router(restarted);
    let persisted = respond(
        &restarted_app,
        browser_request(
            Method::GET,
            "/api/v2/admin/overview",
            json!({}),
            Some(&cookie),
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(persisted.status(), StatusCode::OK);

    let password_change = respond(
        &restarted_app,
        browser_request(
            Method::POST,
            "/api/v2/admin/password",
            json!({
                "current_password": ADMIN_PASSWORD,
                "new_password": NEW_ADMIN_PASSWORD
            }),
            Some(&cookie),
            Some(&csrf),
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(password_change.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        respond(
            &restarted_app,
            browser_request(
                Method::GET,
                "/api/v2/admin/overview",
                json!({}),
                Some(&cookie),
                None,
                "http://localhost",
            ),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let old_password = respond(
        &restarted_app,
        browser_request(
            Method::POST,
            "/api/v2/auth/login",
            json!({"username": ADMIN_USERNAME, "password": ADMIN_PASSWORD}),
            None,
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(old_password.status(), StatusCode::UNAUTHORIZED);
    let persisted_role_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('auth_users') WHERE name = 'role'",
    )
    .fetch_one(&restarted_pool)
    .await
    .unwrap();
    assert_eq!(persisted_role_columns, 0);

    let (active_cookie, _) = login(&restarted_app, NEW_ADMIN_PASSWORD).await;
    sqlx::query("UPDATE auth_users SET active = 0")
        .execute(&restarted_pool)
        .await
        .unwrap();
    assert_eq!(
        respond(
            &restarted_app,
            browser_request(
                Method::GET,
                "/api/v2/admin/overview",
                json!({}),
                Some(&active_cookie),
                None,
                "http://localhost",
            ),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn idle_and_absolute_expiry_are_enforced_from_persisted_state() {
    let workspace = TestWorkspace::new();
    let (state, pool) = test_state(&workspace).await;
    let app = router(state);
    let (cookie, _) = login(&app, ADMIN_PASSWORD).await;
    sqlx::query(
        "UPDATE auth_sessions SET created_at = 0, last_seen_at = 0, \
         idle_expires_at = 1, absolute_expires_at = 1",
    )
    .execute(&pool)
    .await
    .unwrap();
    let expired = respond(
        &app,
        browser_request(
            Method::GET,
            "/api/v2/admin/overview",
            json!({}),
            Some(&cookie),
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn production_cookie_uses_host_prefix_and_complete_security_attributes() {
    let workspace = TestWorkspace::new();
    let (mut state, _) = test_state(&workspace).await;
    state.config.development = false;
    let app = router(state);
    let response = respond(
        &app,
        browser_request(
            Method::POST,
            "/api/v2/auth/login",
            json!({"username": ADMIN_USERNAME, "password": ADMIN_PASSWORD}),
            None,
            None,
            "https://localhost",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cookie.starts_with("__Host-media_session="));
    assert!(cookie.contains("; Path=/"));
    assert!(cookie.contains("; Secure"));
    assert!(cookie.contains("; HttpOnly"));
    assert!(cookie.contains("; SameSite=Strict"));
    assert!(!cookie.contains("Domain="));
}

#[tokio::test]
async fn login_body_limit_and_source_and_normalized_account_budgets_are_enforced() {
    let workspace = TestWorkspace::new();
    let (mut state, _) = test_state(&workspace).await;
    state.login_admission =
        crate::login_admission::LoginAdmission::for_test(2, 2, 1, Duration::from_millis(100));
    let app = router(state);

    let mut oversized = Request::builder()
        .method(Method::POST)
        .uri("/api/v2/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::HOST, "localhost")
        .header(header::ORIGIN, "http://localhost")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(format!(
            "{{\"username\":\"admin\",\"password\":\"{}\"}}",
            "x".repeat(5_000)
        )))
        .unwrap();
    oversized.extensions_mut().insert(ConnectInfo(
        "192.0.2.1:43000".parse::<std::net::SocketAddr>().unwrap(),
    ));
    assert_eq!(
        respond(&app, oversized).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );

    for index in 0..2 {
        let mut request = browser_request_from(
            "198.51.100.10:43000".parse().unwrap(),
            Method::POST,
            "/api/v2/auth/login",
            json!({"username": format!("unknown-{index}"), "password": "incorrect-password"}),
            None,
            None,
            "http://localhost",
        );
        request.headers_mut().insert(
            "x-forwarded-for",
            format!("203.0.113.{}", index + 1).parse().unwrap(),
        );
        assert_eq!(
            respond(&app, request).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let mut source_limited = browser_request_from(
        "198.51.100.10:43000".parse().unwrap(),
        Method::POST,
        "/api/v2/auth/login",
        json!({"username": "another-unknown", "password": "incorrect-password"}),
        None,
        None,
        "http://localhost",
    );
    source_limited
        .headers_mut()
        .insert("x-forwarded-for", "203.0.113.99".parse().unwrap());
    let source_limited = respond(&app, source_limited).await;
    assert_eq!(source_limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(source_limited.headers().contains_key(header::RETRY_AFTER));

    let workspace = TestWorkspace::new();
    let (mut state, _) = test_state(&workspace).await;
    state.login_admission =
        crate::login_admission::LoginAdmission::for_test(10, 2, 1, Duration::from_millis(100));
    let app = router(state);
    for (peer, username) in [
        ("192.0.2.11:44000", " PERSISTED-ADMIN "),
        ("192.0.2.12:44000", "persisted-admin"),
    ] {
        let response = respond(
            &app,
            browser_request_from(
                peer.parse().unwrap(),
                Method::POST,
                "/api/v2/auth/login",
                json!({"username": username, "password": "incorrect-password"}),
                None,
                None,
                "http://localhost",
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let account_limited = respond(
        &app,
        browser_request_from(
            "192.0.2.13:44000".parse().unwrap(),
            Method::POST,
            "/api/v2/auth/login",
            json!({"username": "Persisted-Admin", "password": "incorrect-password"}),
            None,
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(account_limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(account_limited.headers().contains_key(header::RETRY_AFTER));
}

#[tokio::test]
async fn trusted_peer_is_required_for_forwarded_https_and_client_identity() {
    let workspace = TestWorkspace::new();
    let (mut state, pool) = test_state(&workspace).await;
    state.config.require_https = true;
    state.config.development = false;
    state.config.trusted_proxy_cidrs = vec!["127.0.0.1/32".parse().unwrap()];
    let app = router(state);

    let mut spoofed = browser_request_from(
        "198.51.100.20:45000".parse().unwrap(),
        Method::POST,
        "/api/v2/auth/login",
        json!({"username": ADMIN_USERNAME, "password": ADMIN_PASSWORD}),
        None,
        None,
        "https://localhost",
    );
    spoofed
        .headers_mut()
        .insert("x-forwarded-proto", "https".parse().unwrap());
    spoofed
        .headers_mut()
        .insert("x-forwarded-for", "203.0.113.50".parse().unwrap());
    assert_eq!(
        respond(&app, spoofed).await.status(),
        StatusCode::UPGRADE_REQUIRED
    );

    let mut trusted = browser_request_from(
        "127.0.0.1:45000".parse().unwrap(),
        Method::POST,
        "/api/v2/auth/login",
        json!({"username": ADMIN_USERNAME, "password": ADMIN_PASSWORD}),
        None,
        None,
        "https://localhost",
    );
    trusted
        .headers_mut()
        .insert("x-forwarded-proto", "https".parse().unwrap());
    trusted
        .headers_mut()
        .insert("x-forwarded-for", "203.0.113.50".parse().unwrap());
    assert_eq!(respond(&app, trusted).await.status(), StatusCode::OK);
    let created_ip: String =
        sqlx::query_scalar("SELECT created_ip FROM auth_sessions ORDER BY created_at DESC LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(created_ip, "203.0.113.50");
}

#[tokio::test]
async fn device_bootstrap_shares_the_bounded_password_admission_and_body_limit() {
    let workspace = TestWorkspace::new();
    let (mut state, _) = test_state(&workspace).await;
    state.login_admission =
        crate::login_admission::LoginAdmission::for_test(1, 8, 1, Duration::from_millis(100));
    let app = router(state);

    let first = respond(
        &app,
        browser_request_from(
            "192.0.2.30:46000".parse().unwrap(),
            Method::POST,
            "/v2/auth/bootstrap",
            json!({
                "username": "unknown-device-user",
                "password": "incorrect-password",
                "device_name": "Phone",
                "platform": "test"
            }),
            None,
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(first.status(), StatusCode::UNAUTHORIZED);
    let limited = respond(
        &app,
        browser_request_from(
            "192.0.2.30:46000".parse().unwrap(),
            Method::POST,
            "/v2/auth/bootstrap",
            json!({
                "username": "different-device-user",
                "password": "incorrect-password",
                "device_name": "Phone",
                "platform": "test"
            }),
            None,
            None,
            "http://localhost",
        ),
    )
    .await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().contains_key(header::RETRY_AFTER));

    let workspace = TestWorkspace::new();
    let (state, _) = test_state(&workspace).await;
    let app = router(state);
    let mut oversized = Request::builder()
        .method(Method::POST)
        .uri("/v2/auth/bootstrap")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            "{{\"username\":\"u\",\"password\":\"{}\",\"device_name\":\"p\",\"platform\":\"t\"}}",
            "x".repeat(5_000)
        )))
        .unwrap();
    oversized.extensions_mut().insert(ConnectInfo(
        "192.0.2.31:46000".parse::<std::net::SocketAddr>().unwrap(),
    ));
    assert_eq!(
        respond(&app, oversized).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
