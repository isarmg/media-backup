use std::{path::Path, path::PathBuf, str::FromStr};

use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Method, Request, Response, StatusCode},
    Router,
};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tower::ServiceExt;
use uuid::Uuid;

use photo_backup_protocol::{CreateUploadRequest, MediaKind, StorageEncoding, UploadPartSpec};

use crate::{
    config::Config,
    database,
    routes::{router, AppState},
    storage::LocalStorage,
    upload_commit,
    upload_commit::CommitFailpoint,
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

fn database_url(path: &Path) -> String {
    format!("sqlite://{}", path.display())
}

async fn test_state(database: &Path, data: &Path) -> (AppState, SqlitePool) {
    let database_url = database_url(database);
    let pool = database::connect(&database_url)
        .await
        .expect("connect test SQLite");
    let storage = LocalStorage::new(data.to_path_buf())
        .await
        .expect("create test storage");
    let config = Config {
        database_url,
        data_dir: data.to_path_buf(),
        bind: "127.0.0.1:0".parse().expect("test bind address"),
        admin_username: ADMIN_USERNAME.to_owned(),
        admin_password: ADMIN_PASSWORD.to_owned(),
        max_part_bytes: 1024 * 1024,
        metrics_token: None,
        require_https: false,
        development: true,
        admin_session_idle_seconds: 1_800,
        admin_session_absolute_seconds: 43_200,
        trusted_proxy_cidrs: Vec::new(),
    };
    let state = AppState {
        pool: pool.clone(),
        storage,
        config,
        login_admission: crate::login_admission::LoginAdmission::default(),
    };
    crate::admin::ensure_admin_user(&state)
        .await
        .expect("bootstrap persisted test administrator");
    upload_commit::reconcile_all(&state)
        .await
        .expect("reconcile uploads on test startup");
    (state, pool)
}

#[tokio::test]
async fn production_pool_applies_pragmas_and_persists_after_reopen() {
    let workspace = TestWorkspace::new();
    let database_path = workspace.database();
    assert!(!database_path.exists());

    let database_url = database_url(&database_path);
    let pool = database::connect(&database_url)
        .await
        .expect("connect production SQLite pool");
    assert!(database_path.is_file());

    let mut first = pool.acquire().await.expect("acquire first connection");
    let mut second = pool.acquire().await.expect("acquire second connection");
    for connection in [&mut *first, &mut *second] {
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut *connection)
            .await
            .expect("read journal mode");
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut *connection)
            .await
            .expect("read foreign key setting");
        let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut *connection)
            .await
            .expect("read busy timeout");
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut *connection)
            .await
            .expect("read synchronous setting");

        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(foreign_keys, 1);
        assert_eq!(busy_timeout, 5_000);
        assert_eq!(synchronous, 2);
    }
    drop(first);
    drop(second);

    let invalid_foreign_key = sqlx::query(
        "INSERT INTO devices(\
             id, account_id, name, platform, token_hash, created_at, last_seen_at\
         ) VALUES (?, ?, 'invalid', 'test', ?, datetime('now'), datetime('now'))",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(vec![7_u8; 32])
    .execute(&pool)
    .await;
    assert!(
        matches!(invalid_foreign_key, Err(sqlx::Error::Database(_))),
        "foreign key enforcement must reject an orphan device"
    );

    let account_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO accounts(\
             id, username, display_name, storage_path, quota_bytes, enabled, created_at\
         ) VALUES (?, 'persistent-user', 'Persistent User', 'blobs/persistent', 1, true, datetime('now'))",
    )
    .bind(account_id)
    .execute(&pool)
    .await
    .expect("insert persistent account");

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

    pool.close().await;
    let reopened = database::connect(&database_url)
        .await
        .expect("reopen production SQLite pool");
    let persisted: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM accounts WHERE id = ? AND username = ?")
            .bind(account_id)
            .bind("persistent-user")
            .fetch_one(&reopened)
            .await
            .expect("read persistent account");
    assert_eq!(persisted, 1);
    reopened.close().await;
}

fn json_request(
    method: Method,
    uri: impl AsRef<str>,
    body: Value,
    bearer: Option<&str>,
    browser: Option<(&str, &str)>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri.as_ref())
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::HOST, "photos.test")
        .header(header::ORIGIN, "http://photos.test");
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some((cookie, csrf)) = browser {
        builder = builder
            .header(header::COOKIE, cookie)
            .header("x-csrf-token", csrf);
    }
    let mut request = builder
        .body(Body::from(body.to_string()))
        .expect("build JSON request");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:41000"
            .parse::<std::net::SocketAddr>()
            .expect("test peer address"),
    ));
    request
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
    let request_description = format!("{} {}", request.method(), request.uri());
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
            "unexpected response status for {request_description}: expected {expected}, got {status}: {}",
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

    send(
        &app,
        json_request(
            Method::POST,
            "/v1/auth/bootstrap",
            json!({
                "username": USERNAME,
                "password": PASSWORD,
                "device_name": "Old Route Client",
                "platform": "test"
            }),
            None,
            None,
        ),
        StatusCode::NOT_FOUND,
    )
    .await;

    let login = send(
        &app,
        json_request(
            Method::POST,
            "/admin/api/login",
            json!({"username": ADMIN_USERNAME, "password": ADMIN_PASSWORD}),
            None,
            None,
        ),
        StatusCode::OK,
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
    let admin_csrf = json_body(login).await["csrf_token"]
        .as_str()
        .expect("admin CSRF token")
        .to_owned();

    for (index, invalid_storage_path) in [
        "/tmp/photo-backup-outside",
        "../photo-backup-outside",
        "blobs//invalid",
        "uploads/account",
    ]
    .into_iter()
    .enumerate()
    {
        send(
            &app,
            json_request(
                Method::POST,
                "/admin/api/users",
                json!({
                    "username": format!("invalid-path-{index}"),
                    "password": PASSWORD,
                    "display_name": "Invalid Storage Path",
                    "storage_path": invalid_storage_path,
                    "quota_bytes": 1,
                    "enabled": true
                }),
                None,
                Some((&admin_cookie, &admin_csrf)),
            ),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }

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
                Some((&admin_cookie, &admin_csrf)),
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
                "username": "nested-storage-owner",
                "password": PASSWORD,
                "display_name": "Nested Storage Owner",
                "storage_path": format!("{storage_path}/nested"),
                "quota_bytes": 1,
                "enabled": true
            }),
            None,
            Some((&admin_cookie, &admin_csrf)),
        ),
        StatusCode::CONFLICT,
    )
    .await;

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
            Some((&admin_cookie, &admin_csrf)),
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
            Some((&admin_cookie, &admin_csrf)),
        ),
        StatusCode::OK,
    )
    .await;

    let bootstrap = json_body(
        send(
            &app,
            json_request(
                Method::POST,
                "/v2/auth/bootstrap",
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
                "/v2/uploads",
                json!({
                    "source_asset_id": "asset-1",
                    "source_resource_id": "resource-1",
                    "media_kind": "photo",
                    "role": "primary",
                    "filename": "photo.jpg",
                    "mime_type": "image/jpeg",
                    "source_created_at_ms": 1_750_000_000_000_i64,
                    "storage_encoding": "plain-v1",
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
            format!("/v2/uploads/{upload_id}/parts/0"),
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
                format!("/v2/uploads/{upload_id}/complete"),
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
    let resource_id = Uuid::from_str(
        completed["resource_id"]
            .as_str()
            .expect("completed resource id"),
    )
    .expect("valid resource id");
    let content_response = send(
        &app,
        authorized_request(
            Method::GET,
            format!("/v2/resources/{resource_id}/content"),
            &bearer,
            Body::empty(),
        ),
        StatusCode::OK,
    )
    .await;
    let downloaded = to_bytes(content_response.into_body(), 1024 * 1024)
        .await
        .expect("read downloaded blob");
    assert_eq!(downloaded.as_ref(), content);
    let persisted_blob_key: String =
        sqlx::query_scalar("SELECT storage_path FROM blobs WHERE account_id = ?")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("read persisted blob key");
    assert!(persisted_blob_key.starts_with(&format!("{storage_path}/")));
    assert!(!Path::new(&persisted_blob_key).is_absolute());

    let default_timeline = get_json(&app, "/v2/timeline", &bearer).await;
    assert_eq!(default_timeline["items"].as_array().map(Vec::len), Some(1));

    let updated_asset = json_body(
        send(
            &app,
            json_request(
                Method::PATCH,
                format!("/v2/assets/{asset_id}"),
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
            format!("/v2/assets/{asset_id}/trash"),
            &bearer,
            Body::empty(),
        ),
        StatusCode::NO_CONTENT,
    )
    .await;
    let trashed = get_json(&app, "/v2/timeline?trashed=true", &bearer).await;
    assert_eq!(trashed["items"].as_array().map(Vec::len), Some(1));
    send(
        &app,
        authorized_request(
            Method::POST,
            format!("/v2/assets/{asset_id}/restore"),
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
                "/v2/albums",
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
                "/v2/tags",
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
            format!("/v2/tags/{tag_id}/assets"),
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
            format!("/v2/tags/{tag_id}/assets/{asset_id}"),
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
            format!("/v2/tags/{tag_id}/assets/{asset_id}"),
            &bearer,
            Body::empty(),
        ),
        StatusCode::NO_CONTENT,
    )
    .await;

    let filtered_timeline = get_json(
        &app,
        format!("/v2/timeline?favorite=true&archived=true&album_id={album_id}&tag_id={tag_id}"),
        &bearer,
    )
    .await;
    assert_eq!(filtered_timeline["items"].as_array().map(Vec::len), Some(1));

    let api_key = json_body(
        send(
            &app,
            json_request(
                Method::POST,
                "/v2/api-keys",
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
    let api_keys = get_json(&app, "/v2/api-keys", &api_token).await;
    assert_eq!(api_keys.as_array().map(Vec::len), Some(1));

    let first_audit_page = get_json(&app, "/v2/audit-events?limit=2", &bearer).await;
    assert_eq!(first_audit_page["events"].as_array().map(Vec::len), Some(2));
    let before = first_audit_page["next_sequence"]
        .as_i64()
        .expect("audit pagination cursor");
    let second_audit_page = get_json(
        &app,
        format!("/v2/audit-events?limit=2&before={before}"),
        &bearer,
    )
    .await;
    assert!(!second_audit_page["events"]
        .as_array()
        .expect("second audit page")
        .is_empty());
    let changes = get_json(&app, "/v2/sync?after=0", &bearer).await;
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
    let persisted_timeline = get_json(&restarted_app, "/v2/timeline", &bearer).await;
    assert_eq!(
        persisted_timeline["items"].as_array().map(Vec::len),
        Some(1)
    );
    drop(restarted_app);
    restarted_pool.close().await;
}

async fn seed_account(pool: &SqlitePool, storage_path: &str, suffix: &str) -> (Uuid, Uuid) {
    let account_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO accounts(
            id, username, display_name, storage_path, quota_bytes, enabled, created_at
        ) VALUES (?, ?, ?, ?, 100000000, TRUE, datetime('now'))
        "#,
    )
    .bind(account_id)
    .bind(format!("commit-account-{suffix}"))
    .bind(format!("Commit Account {suffix}"))
    .bind(storage_path)
    .execute(pool)
    .await
    .expect("insert commit test account");
    let device_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO devices(
            id, account_id, name, platform, token_hash, created_at, last_seen_at
        ) VALUES (?, ?, ?, 'test', ?, datetime('now'), datetime('now'))
        "#,
    )
    .bind(device_id)
    .bind(account_id)
    .bind(format!("Commit Device {suffix}"))
    .bind(
        blake3::hash(format!("token-{suffix}").as_bytes())
            .as_bytes()
            .to_vec(),
    )
    .execute(pool)
    .await
    .expect("insert commit test device");
    (account_id, device_id)
}

async fn seed_received_upload(
    state: &AppState,
    account_id: Uuid,
    device_id: Uuid,
    content: &[u8],
    suffix: &str,
) -> Uuid {
    let asset_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO assets(
            id, account_id, device_id, source_asset_id, media_kind, source_created_at_ms,
            created_at, updated_at
        ) VALUES (?, ?, ?, ?, 'photo', 1, datetime('now'), datetime('now'))
        "#,
    )
    .bind(asset_id)
    .bind(account_id)
    .bind(device_id)
    .bind(format!("commit-asset-{suffix}"))
    .execute(&state.pool)
    .await
    .expect("insert commit test asset");

    let content_hash = blake3::hash(content).to_hex().to_string();
    let part = UploadPartSpec {
        index: 0,
        size: content.len() as u64,
        blake3: content_hash.clone(),
    };
    let request = CreateUploadRequest {
        source_asset_id: format!("commit-asset-{suffix}"),
        source_resource_id: format!("commit-resource-{suffix}"),
        media_kind: MediaKind::Photo,
        role: "primary".to_owned(),
        filename: format!("{suffix}.jpg"),
        mime_type: "image/jpeg".to_owned(),
        source_created_at_ms: 1,
        storage_encoding: StorageEncoding::PlainV1,
        content_size: content.len() as u64,
        content_blake3: content_hash,
        metadata: None,
        parts: vec![part.clone()],
    };
    let upload_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO uploads(
            id, account_id, device_id, asset_id, source_resource_id, content_blake3, request,
            created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'))
        "#,
    )
    .bind(upload_id)
    .bind(account_id)
    .bind(device_id)
    .bind(asset_id)
    .bind(&request.source_resource_id)
    .bind(&request.content_blake3)
    .bind(serde_json::to_value(&request).expect("serialize commit request"))
    .execute(&state.pool)
    .await
    .expect("insert commit test upload");
    sqlx::query(
        r#"
        INSERT INTO upload_parts(
            upload_id, part_index, expected_size, expected_blake3
        ) VALUES (?, 0, ?, ?)
        "#,
    )
    .bind(upload_id)
    .bind(i64::try_from(part.size).expect("part size fits SQLite"))
    .bind(&part.blake3)
    .execute(&state.pool)
    .await
    .expect("insert commit test part");
    state
        .storage
        .put_part(upload_id, &part, Body::from(content.to_vec()), 1024 * 1024)
        .await
        .expect("persist commit test part");
    sqlx::query(
        "UPDATE upload_parts SET received_size = expected_size, received_at = datetime('now') WHERE upload_id = ? AND part_index = 0",
    )
    .bind(upload_id)
    .execute(&state.pool)
    .await
    .expect("mark commit test part received");
    upload_id
}

async fn close_test_state(state: AppState, pool: SqlitePool) {
    drop(pool);
    state.pool.close().await;
}

#[tokio::test]
async fn upload_commit_recovers_every_durable_crash_boundary_after_restart() {
    let workspace = TestWorkspace::new();
    let (mut state, mut pool) = test_state(&workspace.database(), &workspace.data()).await;
    let (account_id, device_id) = seed_account(&pool, "blobs/recovery", "recovery").await;

    for (index, failpoint) in [
        CommitFailpoint::CommitStarted,
        CommitFailpoint::StageFsync,
        CommitFailpoint::Finalizing,
        CommitFailpoint::Published,
        CommitFailpoint::MetadataCommitted,
    ]
    .into_iter()
    .enumerate()
    {
        let content = format!("restart-safe-upload-{index}-{}", "x".repeat(128 * 1024));
        let upload_id = seed_received_upload(
            &state,
            account_id,
            device_id,
            content.as_bytes(),
            &format!("restart-{index}"),
        )
        .await;
        let error =
            upload_commit::complete_with_failpoint(&state, upload_id, account_id, failpoint)
                .await
                .expect_err("failpoint simulates process death");
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        let resources_before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM resources r JOIN uploads u ON u.asset_id = r.asset_id WHERE u.id = ?",
        )
        .bind(upload_id)
        .fetch_one(&pool)
        .await
        .expect("count pre-recovery resources");
        let blobs_before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blobs b JOIN uploads u ON u.account_id = b.account_id AND u.commit_blob_id = b.id WHERE u.id = ?",
        )
        .bind(upload_id)
        .fetch_one(&pool)
        .await
        .expect("count pre-recovery blobs");
        let expected_metadata = i64::from(failpoint == CommitFailpoint::MetadataCommitted);
        assert_eq!(
            resources_before, expected_metadata,
            "resource must appear exactly with the proven metadata transaction"
        );
        assert_eq!(
            blobs_before, expected_metadata,
            "quota-bearing blob must appear exactly with the proven metadata transaction"
        );

        close_test_state(state, pool).await;
        (state, pool) = test_state(&workspace.database(), &workspace.data()).await;
        let commit_state: String =
            sqlx::query_scalar("SELECT commit_state FROM uploads WHERE id = ?")
                .bind(upload_id)
                .fetch_one(&pool)
                .await
                .expect("read recovered commit state");
        assert_eq!(commit_state, "committed");
        let counts: (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT COUNT(*) FROM blobs b JOIN uploads u ON u.commit_blob_id = b.id WHERE u.id = ?1),
                (SELECT COUNT(*) FROM resources r JOIN uploads u ON u.commit_resource_id = r.id WHERE u.id = ?1)
            "#,
        )
        .bind(upload_id)
        .fetch_one(&pool)
        .await
        .expect("count recovered metadata");
        assert_eq!(counts, (1, 1));
        let (staged_key, final_key): (Option<String>, String) =
            sqlx::query_as("SELECT commit_staged_key, commit_final_key FROM uploads WHERE id = ?")
                .bind(upload_id)
                .fetch_one(&pool)
                .await
                .expect("read recovered object keys");
        if let Some(staged_key) = staged_key {
            assert!(!workspace.data().join(staged_key).exists());
        }
        assert!(workspace.data().join(final_key).is_file());
    }
    close_test_state(state, pool).await;
}

#[tokio::test]
async fn upload_commit_serializes_retries_and_deduplicates_concurrent_uploads() {
    let workspace = TestWorkspace::new();
    let (state, pool) = test_state(&workspace.database(), &workspace.data()).await;
    let (account_id, device_id) = seed_account(&pool, "blobs/concurrent", "concurrent").await;
    let content = vec![91_u8; 256 * 1024 + 17];
    let upload_id =
        seed_received_upload(&state, account_id, device_id, &content, "same-upload").await;
    let first_state = state.clone();
    let second_state = state.clone();
    let (first, second) = tokio::join!(
        upload_commit::complete(&first_state, upload_id, account_id),
        upload_commit::complete(&second_state, upload_id, account_id),
    );
    let first = first.expect("first retry completes");
    let second = second.expect("second retry is idempotent");
    assert_eq!(first.resource_id, second.resource_id);

    let upload_two =
        seed_received_upload(&state, account_id, device_id, &content, "same-content-two").await;
    let upload_three = seed_received_upload(
        &state,
        account_id,
        device_id,
        &content,
        "same-content-three",
    )
    .await;
    let state_two = state.clone();
    let state_three = state.clone();
    let (two, three) = tokio::join!(
        upload_commit::complete(&state_two, upload_two, account_id),
        upload_commit::complete(&state_three, upload_three, account_id),
    );
    two.expect("first concurrent content commit");
    three.expect("second concurrent content commit");
    let blob_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM blobs WHERE account_id = ? AND content_blake3 = ?",
    )
    .bind(account_id)
    .bind(blake3::hash(&content).to_hex().to_string())
    .fetch_one(&pool)
    .await
    .expect("count deduplicated blobs");
    let resource_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM resources WHERE asset_id IN (SELECT asset_id FROM uploads WHERE id IN (?1, ?2, ?3))",
    )
    .bind(upload_id)
    .bind(upload_two)
    .bind(upload_three)
    .fetch_one(&pool)
    .await
    .expect("count independently committed resources");
    assert_eq!(blob_count, 1);
    assert_eq!(resource_count, 3);
    close_test_state(state, pool).await;
}

#[tokio::test]
async fn reconciler_never_accepts_same_size_different_content() {
    let workspace = TestWorkspace::new();
    let (state, pool) = test_state(&workspace.database(), &workspace.data()).await;
    let (account_id, device_id) = seed_account(&pool, "blobs/conflict", "conflict").await;
    let expected = vec![17_u8; 128 * 1024 + 9];
    let conflicting = vec![23_u8; expected.len()];
    let upload_id =
        seed_received_upload(&state, account_id, device_id, &expected, "hash-conflict").await;
    upload_commit::complete_with_failpoint(
        &state,
        upload_id,
        account_id,
        CommitFailpoint::Finalizing,
    )
    .await
    .expect_err("stop before publication");
    let final_key: String = sqlx::query_scalar("SELECT commit_final_key FROM uploads WHERE id = ?")
        .bind(upload_id)
        .fetch_one(&pool)
        .await
        .expect("read final key");
    let final_path = workspace.data().join(&final_key);
    std::fs::create_dir_all(final_path.parent().expect("final parent"))
        .expect("create conflicting final parent");
    std::fs::write(&final_path, &conflicting).expect("write conflicting final blob");

    close_test_state(state, pool).await;
    let (restarted, restarted_pool) = test_state(&workspace.database(), &workspace.data()).await;
    let commit_state: String = sqlx::query_scalar("SELECT commit_state FROM uploads WHERE id = ?")
        .bind(upload_id)
        .fetch_one(&restarted_pool)
        .await
        .expect("read conflicted commit state");
    assert_eq!(commit_state, "unknown");
    let metadata_counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM blobs WHERE account_id = ?1), (SELECT COUNT(*) FROM resources r JOIN uploads u ON u.asset_id = r.asset_id WHERE u.id = ?2)",
    )
    .bind(account_id)
    .bind(upload_id)
    .fetch_one(&restarted_pool)
    .await
    .expect("count conflicted metadata");
    assert_eq!(metadata_counts, (0, 0));
    assert_eq!(
        std::fs::read(final_path).expect("read preserved conflict"),
        conflicting
    );
    close_test_state(restarted, restarted_pool).await;
}

#[tokio::test]
async fn orphan_cleanup_is_scoped_and_preserves_cross_account_references() {
    let workspace = TestWorkspace::new();
    let (state, pool) = test_state(&workspace.database(), &workspace.data()).await;
    let (account_a, device_a) = seed_account(&pool, "blobs/account-a", "boundary-a").await;
    let (_account_b, _device_b) = seed_account(&pool, "blobs/account-b", "boundary-b").await;
    let content = b"cross-account-stage-proof";
    let upload_id =
        seed_received_upload(&state, account_a, device_a, content, "boundary-upload").await;
    upload_commit::complete_with_failpoint(
        &state,
        upload_id,
        account_a,
        CommitFailpoint::Finalizing,
    )
    .await
    .expect_err("stop with a durable stage");
    let original_stage: String =
        sqlx::query_scalar("SELECT commit_staged_key FROM uploads WHERE id = ?")
            .bind(upload_id)
            .fetch_one(&pool)
            .await
            .expect("read original stage key");
    let foreign_stage = format!(
        "blobs/account-b/staging/commit-{upload_id}-{}.stage",
        Uuid::new_v4()
    );
    let foreign_path = workspace.data().join(&foreign_stage);
    std::fs::create_dir_all(foreign_path.parent().expect("foreign stage parent"))
        .expect("create foreign stage parent");
    std::fs::write(&foreign_path, content).expect("write foreign staged content");
    sqlx::query("UPDATE uploads SET commit_staged_key = ? WHERE id = ?")
        .bind(&foreign_stage)
        .bind(upload_id)
        .execute(&pool)
        .await
        .expect("inject cross-account staged key");
    let orphan = workspace.data().join(format!(
        "blobs/account-a/staging/commit-{}-{}.stage",
        Uuid::new_v4(),
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(orphan.parent().expect("orphan parent")).expect("create orphan parent");
    std::fs::write(&orphan, b"orphan").expect("write orphan stage");

    let report = upload_commit::reconcile_all(&state)
        .await
        .expect("run account-bound reconciler");
    let commit_state: String = sqlx::query_scalar("SELECT commit_state FROM uploads WHERE id = ?")
        .bind(upload_id)
        .fetch_one(&pool)
        .await
        .expect("read boundary state");
    assert_eq!(commit_state, "unknown");
    assert!(
        foreign_path.is_file(),
        "must not delete another account's referenced file"
    );
    assert!(
        !orphan.exists(),
        "unreferenced generated stage should be removed"
    );
    assert!(
        !workspace.data().join(original_stage).exists(),
        "superseded local stage is an orphan and should be removed"
    );
    assert!(report.orphan_stages_removed >= 2);
    close_test_state(state, pool).await;
}
