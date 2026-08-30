use std::{path::Path, path::PathBuf, str::FromStr};

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, Response, StatusCode},
    Router,
};
use serde_json::{json, Value};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    config::Config,
    routes::{router, AppState},
    storage::LocalStorage,
};

const ADMIN_USERNAME: &str = "test-admin";
const ADMIN_PASSWORD: &str = "test-admin-password";
const USERNAME: &str = "photo-owner";
const PASSWORD: &str = "correct-horse-battery-staple";

struct TestWorkspace {
    root: PathBuf,
}

impl TestWorkspace {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("photo-backup-sqlite-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create test workspace");
        Self { root }
    }

    fn database(&self) -> PathBuf {
        self.root.join("photo-backup.db")
    }

    fn data(&self) -> PathBuf {
        self.root.join("data")
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn test_state(database: &Path, data: &Path) -> (AppState, SqlitePool) {
    let options = SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .expect("connect test SQLite");
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .expect("migrate test SQLite");
    let storage = LocalStorage::new(data.to_path_buf())
        .await
        .expect("create test storage");
    let config = Config {
        database_url: database.display().to_string(),
        data_dir: data.to_path_buf(),
        bind: "127.0.0.1:0".parse().expect("test bind address"),
        admin_username: ADMIN_USERNAME.to_owned(),
        admin_password: ADMIN_PASSWORD.to_owned(),
        max_part_bytes: 1024 * 1024,
        metrics_token: None,
        require_https: false,
    };
    (
        AppState {
            pool: pool.clone(),
            storage,
            config,
        },
        pool,
    )
}

fn json_request(
    method: Method,
    uri: impl AsRef<str>,
    body: Value,
    bearer: Option<&str>,
    cookie: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri.as_ref())
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    builder
        .body(Body::from(body.to_string()))
        .expect("build JSON request")
}

fn authorized_request(
    method: Method,
    uri: impl AsRef<str>,
    bearer: &str,
    body: Body,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri.as_ref())
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body(body)
        .expect("build authorized request")
}

async fn send(app: &Router, request: Request<Body>, expected: StatusCode) -> Response<Body> {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("router should respond");
    if response.status() != expected {
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("read error response");
        panic!(
            "unexpected response status: expected {expected}, got {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    response
}

async fn json_body(response: Response<Body>) -> Value {
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read JSON response");
    serde_json::from_slice(&body).expect("decode JSON response")
}

async fn get_json(app: &Router, uri: impl AsRef<str>, bearer: &str) -> Value {
    let response = send(
        app,
        authorized_request(Method::GET, uri, bearer, Body::empty()),
        StatusCode::OK,
    )
    .await;
    json_body(response).await
}

#[tokio::test]
async fn fresh_sqlite_supports_the_core_data_flow_and_restart() {
    let workspace = TestWorkspace::new();
    let (state, pool) = test_state(&workspace.database(), &workspace.data()).await;
    let app = router(state);

    let login = send(
        &app,
        json_request(
            Method::POST,
            "/admin/api/login",
            json!({"username": ADMIN_USERNAME, "password": ADMIN_PASSWORD}),
            None,
            None,
        ),
        StatusCode::NO_CONTENT,
    )
    .await;
    let admin_cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .expect("admin login cookie")
        .to_str()
        .expect("valid admin login cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned();

    let created_account = json_body(
        send(
            &app,
            json_request(
                Method::POST,
                "/admin/api/users",
                json!({
                    "username": USERNAME,
                    "password": PASSWORD,
                    "display_name": "Photo Owner",
                    "storage_path": "",
                    "quota_bytes": 10_000_000,
                    "enabled": true
                }),
                None,
                Some(&admin_cookie),
            ),
            StatusCode::OK,
        )
        .await,
    )
    .await;
    let account_id = Uuid::from_str(created_account["id"].as_str().expect("created account id"))
        .expect("valid account id");
    let storage_path = created_account["storage_path"]
        .as_str()
        .expect("created account storage path")
        .to_owned();

    send(
        &app,
        json_request(
            Method::POST,
            "/admin/api/users",
            json!({
                "username": USERNAME,
                "password": PASSWORD,
                "display_name": "Duplicate",
                "storage_path": "blobs/duplicate",
                "quota_bytes": 1,
                "enabled": true
            }),
            None,
            Some(&admin_cookie),
        ),
        StatusCode::CONFLICT,
    )
    .await;
    send(
        &app,
        json_request(
            Method::PUT,
            format!("/admin/api/users/{account_id}"),
            json!({
                "username": USERNAME,
                "display_name": "Photo Owner Updated",
                "storage_path": storage_path,
                "quota_bytes": 10_000_000,
                "enabled": true
            }),
            None,
            Some(&admin_cookie),
        ),
        StatusCode::OK,
    )
    .await;

    let bootstrap = json_body(
        send(
            &app,
            json_request(
                Method::POST,
                "/v1/auth/bootstrap",
                json!({
                    "username": USERNAME,
                    "password": PASSWORD,
                    "device_name": "Test Phone",
                    "platform": "test"
                }),
                None,
                None,
            ),
            StatusCode::OK,
        )
        .await,
    )
    .await;
    assert_eq!(bootstrap["account_id"], account_id.to_string());
    let bearer = bootstrap["bearer_token"]
        .as_str()
        .expect("bootstrap bearer token")
        .to_owned();

    let content = b"photo-backup-sqlite-regression";
    let content_hash = blake3::hash(content).to_hex().to_string();
    let created_upload = json_body(
        send(
            &app,
            json_request(
                Method::POST,
                "/v1/uploads",
                json!({
                    "source_asset_id": "asset-1",
                    "source_resource_id": "resource-1",
                    "media_kind": "photo",
                    "role": "primary",
                    "filename": "photo.jpg",
                    "mime_type": "image/jpeg",
                    "source_created_at_ms": 1_750_000_000_000_i64,
                    "content_size": content.len(),
                    "content_blake3": content_hash,
                    "metadata": {"favorite": false},
                    "parts": [{
                        "index": 0,
                        "size": content.len(),
                        "blake3": content_hash
                    }]
                }),
                Some(&bearer),
                None,
            ),
            StatusCode::OK,
        )
        .await,
    )
    .await;
    let upload_id = Uuid::from_str(
        created_upload["upload_id"]
            .as_str()
            .expect("created upload id"),
    )
    .expect("valid upload id");

    send(
        &app,
        authorized_request(
            Method::PUT,
            format!("/v1/uploads/{upload_id}/parts/0"),
            &bearer,
            Body::from(content.as_slice()),
        ),
        StatusCode::NO_CONTENT,
    )
    .await;
    let completed = json_body(
        send(
            &app,
            authorized_request(
                Method::POST,
                format!("/v1/uploads/{upload_id}/complete"),
                &bearer,
                Body::empty(),
            ),
            StatusCode::OK,
        )
        .await,
    )
    .await;
    let asset_id = Uuid::from_str(completed["asset_id"].as_str().expect("completed asset id"))
        .expect("valid asset id");

    let default_timeline = get_json(&app, "/v1/timeline", &bearer).await;
    assert_eq!(default_timeline["items"].as_array().map(Vec::len), Some(1));

    let updated_asset = json_body(
        send(
            &app,
            json_request(
                Method::PATCH,
                format!("/v1/assets/{asset_id}"),
                json!({"favorite": true, "archived": true}),
                Some(&bearer),
                None,
            ),
            StatusCode::OK,
        )
        .await,
    )
    .await;
    assert_eq!(updated_asset["favorite"], true);
    assert_eq!(updated_asset["archived"], true);

    send(
        &app,
        authorized_request(
            Method::POST,
            format!("/v1/assets/{asset_id}/trash"),
            &bearer,
            Body::empty(),
        ),
        StatusCode::NO_CONTENT,
    )
    .await;
    let trashed = get_json(&app, "/v1/timeline?trashed=true", &bearer).await;
    assert_eq!(trashed["items"].as_array().map(Vec::len), Some(1));
    send(
        &app,
        authorized_request(
            Method::POST,
            format!("/v1/assets/{asset_id}/restore"),
            &bearer,
            Body::empty(),
        ),
        StatusCode::NO_CONTENT,
    )
    .await;

    let album = json_body(
        send(
            &app,
            json_request(
                Method::POST,
                "/v1/albums",
                json!({
                    "source_album_id": "album-1",
                    "name": "Regression Album",
                    "source_asset_ids": ["asset-1"],
                    "replace_members": true
                }),
                Some(&bearer),
                None,
            ),
            StatusCode::OK,
        )
        .await,
    )
    .await;
    let album_id =
        Uuid::from_str(album["album_id"].as_str().expect("album id")).expect("valid album id");

    let tag = json_body(
        send(
            &app,
            json_request(
                Method::POST,
                "/v1/tags",
                json!({"name": "regression"}),
                Some(&bearer),
                None,
            ),
            StatusCode::CREATED,
        )
        .await,
    )
    .await;
    let tag_id = Uuid::from_str(tag["tag_id"].as_str().expect("tag id")).expect("valid tag id");
    send(
        &app,
        json_request(
            Method::PUT,
            format!("/v1/tags/{tag_id}/assets"),
            json!({"asset_ids": [asset_id]}),
            Some(&bearer),
            None,
        ),
        StatusCode::NO_CONTENT,
    )
    .await;
    send(
        &app,
        authorized_request(
            Method::DELETE,
            format!("/v1/tags/{tag_id}/assets/{asset_id}"),
            &bearer,
            Body::empty(),
        ),
        StatusCode::NO_CONTENT,
    )
    .await;
    send(
        &app,
        authorized_request(
            Method::POST,
            format!("/v1/tags/{tag_id}/assets/{asset_id}"),
            &bearer,
            Body::empty(),
        ),
        StatusCode::NO_CONTENT,
    )
    .await;

    let filtered_timeline = get_json(
        &app,
        format!("/v1/timeline?favorite=true&archived=true&album_id={album_id}&tag_id={tag_id}"),
        &bearer,
    )
    .await;
    assert_eq!(filtered_timeline["items"].as_array().map(Vec::len), Some(1));

    let api_key = json_body(
        send(
            &app,
            json_request(
                Method::POST,
                "/v1/api-keys",
                json!({"name": "Regression Key"}),
                Some(&bearer),
                None,
            ),
            StatusCode::CREATED,
        )
        .await,
    )
    .await;
    let api_token = api_key["token"]
        .as_str()
        .expect("created API token")
        .to_owned();
    let api_keys = get_json(&app, "/v1/api-keys", &api_token).await;
    assert_eq!(api_keys.as_array().map(Vec::len), Some(1));

    let first_audit_page = get_json(&app, "/v1/audit-events?limit=2", &bearer).await;
    assert_eq!(first_audit_page["events"].as_array().map(Vec::len), Some(2));
    let before = first_audit_page["next_sequence"]
        .as_i64()
        .expect("audit pagination cursor");
    let second_audit_page = get_json(
        &app,
        format!("/v1/audit-events?limit=2&before={before}"),
        &bearer,
    )
    .await;
    assert!(!second_audit_page["events"]
        .as_array()
        .expect("second audit page")
        .is_empty());
    let changes = get_json(&app, "/v1/sync?after=0", &bearer).await;
    assert!(!changes["events"]
        .as_array()
        .expect("sync events")
        .is_empty());

    for table in [
        "accounts",
        "devices",
        "assets",
        "uploads",
        "blobs",
        "resources",
        "albums",
        "album_assets",
        "tags",
        "tag_assets",
        "api_keys",
        "audit_events",
        "account_changes",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("count {table}: {error}"));
        assert!(count > 0, "expected persisted rows in {table}");
    }
    let foreign_key_violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .expect("check foreign keys");
    assert!(foreign_key_violations.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("check SQLite integrity");
    assert_eq!(integrity, "ok");

    drop(app);
    pool.close().await;

    let (restarted_state, restarted_pool) =
        test_state(&workspace.database(), &workspace.data()).await;
    let restarted_app = router(restarted_state);
    let persisted_timeline = get_json(&restarted_app, "/v1/timeline", &bearer).await;
    assert_eq!(
        persisted_timeline["items"].as_array().map(Vec::len),
        Some(1)
    );
    drop(restarted_app);
    restarted_pool.close().await;
}
