use std::path::{Path, PathBuf};

use axum::{
    body::{to_bytes, Body},
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
        let root = std::env::temp_dir().join(format!("photo-browser-auth-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create browser auth test directory");
        Self(root)
    }

    fn database(&self) -> PathBuf {
        self.0.join("photo.db")
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
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .expect("migrate browser auth SQLite");
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
            metrics_token: None,
            require_https: false,
            development: true,
            admin_session_idle_seconds: 1_800,
            admin_session_absolute_seconds: 43_200,
        },
    };
    admin::ensure_admin_user(&state)
        .await
        .expect("seed persisted administrator");
    (state, pool)
}

fn browser_request(
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
        .header(header::HOST, "photos.test")
        .header(header::ORIGIN, origin);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

async fn respond(app: &Router, request: Request<Body>) -> Response<Body> {
    app.clone().oneshot(request).await.expect("router response")
}

async fn login(app: &Router, password: &str) -> (String, String) {
    let response = respond(
        app,
        browser_request(
            Method::POST,
            "/admin/api/login",
            json!({"username": ADMIN_USERNAME, "password": password}),
            None,
            None,
            "http://photos.test",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
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
    (cookie, body["csrf_token"].as_str().unwrap().to_owned())
}

fn cookie_token(cookie: &str) -> &str {
    cookie.split_once('=').unwrap().1
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
            "/admin/api/logout",
            json!({}),
            Some(&first_cookie),
            None,
            "http://photos.test",
        ),
    )
    .await;
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let cross_session = respond(
        &app,
        browser_request(
            Method::POST,
            "/admin/api/logout",
            json!({}),
            Some(&first_cookie),
            Some(&second_csrf),
            "http://photos.test",
        ),
    )
    .await;
    assert_eq!(cross_session.status(), StatusCode::FORBIDDEN);

    let cross_origin = respond(
        &app,
        browser_request(
            Method::POST,
            "/admin/api/logout",
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
        "/admin/api/overview",
        json!({}),
        Some(&first_cookie),
        None,
        "http://photos.test",
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
        .uri("/v1/timeline")
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
            "/admin/api/logout",
            json!({}),
            Some(&first_cookie),
            Some(&first_csrf),
            "http://photos.test",
        ),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let revoked = respond(
        &app,
        browser_request(
            Method::GET,
            "/admin/api/overview",
            json!({}),
            Some(&first_cookie),
            None,
            "http://photos.test",
        ),
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

    let session = respond(
        &app,
        browser_request(
            Method::GET,
            "/admin/api/session",
            json!({}),
            Some(&second_cookie),
            None,
            "http://photos.test",
        ),
    )
    .await;
    assert_eq!(session.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(session.into_body(), 16 * 1024).await.unwrap()).unwrap();
    let rotated_csrf = body["csrf_token"].as_str().unwrap();
    assert_ne!(rotated_csrf, second_csrf);
    let stale_csrf = respond(
        &app,
        browser_request(
            Method::POST,
            "/admin/api/logout",
            json!({}),
            Some(&second_cookie),
            Some(&second_csrf),
            "http://photos.test",
        ),
    )
    .await;
    assert_eq!(stale_csrf.status(), StatusCode::FORBIDDEN);
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
            "/admin/api/overview",
            json!({}),
            Some(&cookie),
            None,
            "http://photos.test",
        ),
    )
    .await;
    assert_eq!(persisted.status(), StatusCode::OK);

    let password_change = respond(
        &restarted_app,
        browser_request(
            Method::POST,
            "/admin/api/password",
            json!({
                "current_password": ADMIN_PASSWORD,
                "new_password": NEW_ADMIN_PASSWORD
            }),
            Some(&cookie),
            Some(&csrf),
            "http://photos.test",
        ),
    )
    .await;
    assert_eq!(password_change.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        respond(
            &restarted_app,
            browser_request(
                Method::GET,
                "/admin/api/overview",
                json!({}),
                Some(&cookie),
                None,
                "http://photos.test",
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
            "/admin/api/login",
            json!({"username": ADMIN_USERNAME, "password": ADMIN_PASSWORD}),
            None,
            None,
            "http://photos.test",
        ),
    )
    .await;
    assert_eq!(old_password.status(), StatusCode::UNAUTHORIZED);
    let (role_cookie, _) = login(&restarted_app, NEW_ADMIN_PASSWORD).await;

    let before_version: i64 = sqlx::query_scalar("SELECT session_version FROM auth_users LIMIT 1")
        .fetch_one(&restarted_pool)
        .await
        .unwrap();
    sqlx::query("UPDATE auth_users SET role = 'viewer'")
        .execute(&restarted_pool)
        .await
        .unwrap();
    let after_role_version: i64 =
        sqlx::query_scalar("SELECT session_version FROM auth_users LIMIT 1")
            .fetch_one(&restarted_pool)
            .await
            .unwrap();
    assert_eq!(after_role_version, before_version + 1);
    assert_eq!(
        respond(
            &restarted_app,
            browser_request(
                Method::GET,
                "/admin/api/overview",
                json!({}),
                Some(&role_cookie),
                None,
                "http://photos.test",
            ),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );

    sqlx::query("UPDATE auth_users SET role = 'admin'")
        .execute(&restarted_pool)
        .await
        .unwrap();
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
                "/admin/api/overview",
                json!({}),
                Some(&active_cookie),
                None,
                "http://photos.test",
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
            "/admin/api/overview",
            json!({}),
            Some(&cookie),
            None,
            "http://photos.test",
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
            "/admin/api/login",
            json!({"username": ADMIN_USERNAME, "password": ADMIN_PASSWORD}),
            None,
            None,
            "https://photos.test",
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
    assert!(cookie.starts_with("__Host-photo_session="));
    assert!(cookie.contains("; Path=/"));
    assert!(cookie.contains("; Secure"));
    assert!(cookie.contains("; HttpOnly"));
    assert!(cookie.contains("; SameSite=Strict"));
    assert!(!cookie.contains("Domain="));
}
