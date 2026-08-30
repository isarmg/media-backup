use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{ensure, Context};
use photo_backup_protocol::CreateUploadRequest;
use rusqlite::{backup::Backup, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::Config,
    rooted_fs::{RootEntryKind, RootedFs},
    runtime_lock::{sqlite_database_path, RuntimeLock},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

const APPLICATION: &str = "photo-backup";
const FORMAT_VERSION: u32 = 1;
const CURRENT_SCHEMA_VERSION: i64 = 3;
const DATABASE_FILE: &str = "database.sqlite3";
const MANIFEST_FILE: &str = "manifest.json";
const FILES_DIRECTORY: &str = "files";
const MANIFEST_LIMIT: u64 = 16 * 1024 * 1024;

const REQUIRED_TABLES: [&str; 16] = [
    "accounts",
    "devices",
    "assets",
    "blobs",
    "resources",
    "uploads",
    "upload_parts",
    "albums",
    "album_assets",
    "tags",
    "tag_assets",
    "account_changes",
    "api_keys",
    "audit_events",
    "auth_users",
    "auth_sessions",
];

const COUNTED_TABLES: [&str; 16] = [
    "accounts",
    "devices",
    "assets",
    "blobs",
    "resources",
    "uploads",
    "upload_parts",
    "albums",
    "album_assets",
    "tags",
    "tag_assets",
    "account_changes",
    "api_keys",
    "audit_events",
    "auth_users",
    "auth_sessions",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupManifest {
    format_version: u32,
    application: String,
    application_version: String,
    created_at_unix: u64,
    schema_version: i64,
    database: DatabaseManifest,
    record_counts: BTreeMap<String, u64>,
    files: Vec<FileManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseManifest {
    file: String,
    size: u64,
    blake3: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FileManifest {
    path: String,
    size: u64,
    blake3: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BackupSummary {
    pub(crate) files: usize,
    pub(crate) bytes: u64,
    pub(crate) schema_version: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DoctorSummary {
    pub(crate) status: &'static str,
    pub(crate) files: usize,
    pub(crate) blobs: u64,
    pub(crate) uploads: u64,
    pub(crate) schema_version: i64,
}

#[derive(Debug)]
struct DatabaseFacts {
    schema_version: i64,
    record_counts: BTreeMap<String, u64>,
}

struct VerifiedBackup {
    root: PathBuf,
    manifest: BackupManifest,
}

type FileIndex = BTreeMap<String, FileManifest>;

pub(crate) fn create(config: &Config, output: &Path) -> anyhow::Result<BackupSummary> {
    let _lock = RuntimeLock::acquire(&config.database_url, &config.data_dir)?;
    create_locked(config, output)
}

fn create_locked(config: &Config, output: &Path) -> anyhow::Result<BackupSummary> {
    let (database, data_dir) = live_paths(config)?;
    let output = absent_anchored_target(output, "backup output")?;
    ensure!(
        !output.starts_with(&data_dir),
        "backup output must be outside DATA_DIR"
    );
    let (mut pending, staging) = PendingDirectory::create_adjacent(&output, "backup")?;
    let database_output = staging.join(DATABASE_FILE);
    create_private_file(&database_output)?;
    online_backup(&database, &database_output)?;
    let database_facts = verify_database(&database_output)?;

    let files_output = staging.join(FILES_DIRECTORY);
    create_private_directory(&files_output)?;
    let files = copy_data_tree(&data_dir, &files_output)?;
    validate_database_files(&database_output, &files)?;

    let (database_size, database_blake3) = hash_path(&database_output)?;
    let manifest = BackupManifest {
        format_version: FORMAT_VERSION,
        application: APPLICATION.to_owned(),
        application_version: env!("CARGO_PKG_VERSION").to_owned(),
        created_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_secs(),
        schema_version: database_facts.schema_version,
        database: DatabaseManifest {
            file: DATABASE_FILE.to_owned(),
            size: database_size,
            blake3: database_blake3,
        },
        record_counts: database_facts.record_counts,
        files: files.values().cloned().collect(),
    };
    write_manifest(&staging.join(MANIFEST_FILE), &manifest)?;
    verify_backup_root(&staging)?;
    sync_tree(&staging)?;
    fs::rename(&staging, &output).context("publish verified backup directory")?;
    pending.commit();
    sync_parent(&output)?;
    Ok(summary(&manifest))
}

pub(crate) fn verify(config: &Config, input: &Path) -> anyhow::Result<BackupSummary> {
    let _lock = RuntimeLock::acquire(&config.database_url, &config.data_dir)?;
    let verified = verify_backup_root(input)?;
    Ok(summary(&verified.manifest))
}

pub(crate) fn restore(config: &Config, input: &Path) -> anyhow::Result<BackupSummary> {
    let _lock = RuntimeLock::acquire(&config.database_url, &config.data_dir)?;
    restore_locked(config, input, None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestoreFailpoint {
    AfterDatabaseInstall,
    AfterDataInstall,
    SidecarCleanup,
    ParentDirectorySync,
    PostInstallVerification,
    OldGenerationCleanup,
}

fn restore_locked(
    config: &Config,
    input: &Path,
    failpoint: Option<RestoreFailpoint>,
) -> anyhow::Result<BackupSummary> {
    let verified = verify_backup_root(input)?;
    let (database, data_dir) = live_paths_for_restore(config)?;
    ensure!(
        !verified.root.starts_with(&data_dir) && !data_dir.starts_with(&verified.root),
        "backup input and DATA_DIR must not overlap"
    );
    ensure!(
        !database.starts_with(&verified.root),
        "SQLite database must be outside the backup input"
    );
    reject_symlink_or_special_destination(&database, false)?;
    reject_symlink_or_special_destination(&data_dir, true)?;
    prepare_database_destination(&database)?;

    let (mut database_stage, staged_database) = PendingFile::create_adjacent(&database, "restore")?;
    copy_backup_database(&verified, &staged_database)?;
    let (mut data_stage, staged_data) = PendingDirectory::create_adjacent(&data_dir, "restore")?;
    copy_manifest_files(&verified, &staged_data)?;
    let staged_index = index_data_tree(&staged_data)?;
    verify_database(&staged_database)?;
    validate_manifest_files(&verified.manifest, &staged_index)?;
    validate_database_files(&staged_database, &staged_index)?;
    sync_tree(&staged_data)?;
    sync_file_and_parent(&staged_database)?;

    let old_database = adjacent_unique_path(&database, "rollback", false)?;
    let old_data = adjacent_unique_path(&data_dir, "rollback", true)?;
    let had_database = database.exists();
    let had_data = data_dir.exists();
    if had_database {
        fs::rename(&database, &old_database).context("stage current database for rollback")?;
    }
    if had_data {
        if let Err(error) = fs::rename(&data_dir, &old_data) {
            if had_database {
                fs::rename(&old_database, &database)
                    .context("roll back database after DATA_DIR staging failure")?;
                sync_parent(&database)?;
            }
            return Err(error).context("stage current DATA_DIR for rollback");
        }
    }

    let install_result = (|| -> anyhow::Result<()> {
        fs::rename(&staged_database, &database).context("install restored database")?;
        database_stage.commit();
        if failpoint == Some(RestoreFailpoint::AfterDatabaseInstall) {
            anyhow::bail!("injected restore failure after database install");
        }
        fs::rename(&staged_data, &data_dir).context("install restored DATA_DIR")?;
        data_stage.commit();
        if failpoint == Some(RestoreFailpoint::AfterDataInstall) {
            anyhow::bail!("injected restore failure after DATA_DIR install");
        }
        if failpoint == Some(RestoreFailpoint::SidecarCleanup) {
            anyhow::bail!("injected restore failure during SQLite sidecar cleanup");
        }
        remove_sqlite_sidecars(&database)?;
        if failpoint == Some(RestoreFailpoint::ParentDirectorySync) {
            anyhow::bail!("injected restore failure during parent-directory synchronization");
        }
        sync_parent(&database)?;
        sync_parent(&data_dir)?;
        let installed_index = index_data_tree(&data_dir)?;
        verify_database(&database)?;
        validate_manifest_files(&verified.manifest, &installed_index)?;
        validate_database_files(&database, &installed_index)?;
        if failpoint == Some(RestoreFailpoint::PostInstallVerification) {
            anyhow::bail!("injected restore post-verification failure");
        }
        Ok(())
    })();

    if let Err(error) = install_result {
        rollback_install(
            &database,
            &data_dir,
            &old_database,
            &old_data,
            had_database,
            had_data,
        )?;
        return Err(error.context("restore rolled back both database and DATA_DIR"));
    }

    if failpoint == Some(RestoreFailpoint::OldGenerationCleanup) {
        anyhow::bail!(
            "restore installed and verified successfully, but old-generation cleanup is required; do not retry restore"
        );
    }
    let cleanup_result = (|| -> anyhow::Result<()> {
        if had_data {
            fs::remove_dir_all(&old_data)
                .context("remove replaced DATA_DIR after committed restore")?;
        }
        if had_database {
            fs::remove_file(&old_database)
                .context("remove replaced database after committed restore")?;
        }
        sync_parent(&database)?;
        sync_parent(&data_dir)?;
        Ok(())
    })();
    if let Err(error) = cleanup_result {
        return Err(error.context(
            "restore installed and verified successfully; old-generation cleanup is incomplete, any remaining rollback evidence was retained, and restore must not be retried",
        ));
    }
    Ok(summary(&verified.manifest))
}

pub(crate) fn doctor(config: &Config) -> anyhow::Result<DoctorSummary> {
    let _lock = RuntimeLock::acquire(&config.database_url, &config.data_dir)?;
    let (database, data_dir) = live_paths(config)?;
    let facts = verify_database(&database)?;
    database_write_rollback_probe(&database)?;
    storage_write_cleanup_probe(&data_dir)?;
    let files = index_data_tree(&data_dir)?;
    validate_database_files(&database, &files)?;
    Ok(DoctorSummary {
        status: "ok",
        files: files.len(),
        blobs: facts.record_counts.get("blobs").copied().unwrap_or(0),
        uploads: facts.record_counts.get("uploads").copied().unwrap_or(0),
        schema_version: facts.schema_version,
    })
}

fn verify_backup_root(input: &Path) -> anyhow::Result<VerifiedBackup> {
    let root = existing_anchored_directory(input, "backup input")?;
    let rooted = RootedFs::new(&root).context("open backup without following links")?;
    let manifest = read_manifest(&rooted)?;
    ensure!(
        manifest.format_version == FORMAT_VERSION,
        "unsupported backup format"
    );
    ensure!(
        manifest.application == APPLICATION,
        "backup belongs to a different application"
    );
    ensure!(
        manifest.database.file == DATABASE_FILE,
        "backup database name is invalid"
    );
    ensure!(
        manifest.schema_version == CURRENT_SCHEMA_VERSION,
        "backup schema version is unsupported"
    );
    ensure_manifest_is_canonical(&manifest)?;

    let actual_backup_files = walk_files(&rooted)?;
    let mut expected_backup_files =
        BTreeSet::from([MANIFEST_FILE.to_owned(), DATABASE_FILE.to_owned()]);
    expected_backup_files.extend(
        manifest
            .files
            .iter()
            .map(|entry| format!("{FILES_DIRECTORY}/{}", entry.path)),
    );
    ensure!(
        actual_backup_files == expected_backup_files,
        "backup contains missing or unexpected files"
    );

    let verification_destination = std::env::temp_dir().join("photo-backup-verification");
    let (_verification_guard, verification_root) =
        PendingDirectory::create_adjacent(&verification_destination, "verify")?;
    let database_path = verification_root.join(DATABASE_FILE);
    let mut database_input = rooted
        .open_read_std(Path::new(DATABASE_FILE))
        .context("open backup database without following links")?;
    let mut database_output = create_private_file(&database_path)?;
    let (database_size, database_hash) = copy_and_hash(&mut database_input, &mut database_output)?;
    ensure!(
        database_size == manifest.database.size && database_hash == manifest.database.blake3,
        "backup database hash does not match manifest"
    );
    database_output.sync_all()?;
    drop(database_output);
    let facts = verify_database(&database_path)?;
    ensure!(
        facts.schema_version == manifest.schema_version
            && facts.record_counts == manifest.record_counts,
        "backup database facts do not match manifest"
    );
    let data_root = root.join(FILES_DIRECTORY);
    let files = index_data_tree(&data_root)?;
    validate_manifest_files(&manifest, &files)?;
    validate_database_files(&database_path, &files)?;
    Ok(VerifiedBackup { root, manifest })
}

fn verify_database(path: &Path) -> anyhow::Result<DatabaseFacts> {
    ensure_regular_file(path, "SQLite database")?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("open SQLite database for verification")?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .context("run SQLite integrity_check")?;
    ensure!(
        integrity.eq_ignore_ascii_case("ok"),
        "SQLite integrity_check failed"
    );
    let mut foreign_keys = connection.prepare("PRAGMA foreign_key_check")?;
    ensure!(
        foreign_keys.query([])?.next()?.is_none(),
        "SQLite foreign_key_check failed"
    );
    let failed_migrations: i64 = connection.query_row(
        "SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 0",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        failed_migrations == 0,
        "database contains a failed migration"
    );
    let schema_version: i64 = connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        schema_version == CURRENT_SCHEMA_VERSION,
        "database is not the current Photo Backup schema"
    );
    let required = REQUIRED_TABLES
        .iter()
        .map(|table| format!("'{table}'"))
        .collect::<Vec<_>>()
        .join(",");
    let table_count: i64 = connection.query_row(
        &format!("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ({required})"),
        [],
        |row| row.get(0),
    )?;
    ensure!(
        table_count == i64::try_from(REQUIRED_TABLES.len())?,
        "database is missing required Photo Backup tables"
    );
    let upload_columns: i64 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('uploads') WHERE name IN (\
         'commit_state','commit_staged_key','commit_final_key','commit_account_path',\
         'commit_expected_size','commit_expected_blake3','commit_blob_id','commit_resource_id',\
         'commit_deduplicated','commit_error','commit_started_at','finalized_at')",
        [],
        |row| row.get(0),
    )?;
    let auth_columns: i64 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('auth_sessions') WHERE name IN (\
         'id','user_id','token_hash','csrf_hash','user_session_version','created_at',\
         'last_seen_at','idle_expires_at','absolute_expires_at','revoked_at')",
        [],
        |row| row.get(0),
    )?;
    let security_trigger: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name='auth_users_security_version'",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        upload_columns == 12 && auth_columns == 10 && security_trigger == 1,
        "database is missing upload recovery or browser authentication schema"
    );
    let mut record_counts = BTreeMap::new();
    for table in COUNTED_TABLES {
        let count: i64 =
            connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })?;
        record_counts.insert(table.to_owned(), u64::try_from(count)?);
    }
    Ok(DatabaseFacts {
        schema_version,
        record_counts,
    })
}

fn validate_database_files(database: &Path, files: &FileIndex) -> anyhow::Result<()> {
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    validate_account_storage_paths(&connection)?;
    validate_upload_manifests(&connection, files)?;

    let unknown_uploads: i64 = connection.query_row(
        "SELECT COUNT(*) FROM uploads WHERE commit_state = 'unknown'",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        unknown_uploads == 0,
        "an upload has unknown commit state; reconcile it before backup"
    );
    let broken_committed: i64 = connection.query_row(
        "SELECT COUNT(*) FROM uploads u \
         LEFT JOIN accounts a ON a.id = u.account_id \
         LEFT JOIN blobs b ON b.id = u.commit_blob_id \
         LEFT JOIN resources r ON r.id = u.commit_resource_id \
         WHERE u.commit_state = 'committed' AND ( \
             u.state <> 'complete' OR u.completed_at IS NULL OR \
             u.commit_blob_id IS NULL OR u.commit_resource_id IS NULL OR \
             u.commit_final_key IS NULL OR u.commit_account_path IS NULL OR \
             u.commit_expected_size IS NULL OR u.commit_expected_blake3 IS NULL OR \
             b.id IS NULL OR b.account_id <> u.account_id OR \
             b.storage_path <> u.commit_final_key OR b.stored_size <> u.commit_expected_size OR \
             b.content_blake3 IS NULL OR b.content_blake3 <> u.commit_expected_blake3 OR \
             b.storage_encoding <> 'plain-v1' OR \
             r.id IS NULL OR r.blob_id <> b.id OR r.asset_id <> u.asset_id OR \
             a.id IS NULL OR a.storage_path <> u.commit_account_path \
         )",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        broken_committed == 0,
        "a committed upload has incomplete metadata"
    );

    let mut blobs = connection.prepare(
        "SELECT a.storage_path, b.storage_path, b.stored_size, b.content_blake3, b.storage_encoding \
         FROM blobs b JOIN accounts a ON a.id = b.account_id ORDER BY b.id",
    )?;
    let rows = blobs.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    for row in rows {
        let (account_path, storage_path, stored_size, content_hash, encoding) = row?;
        ensure_scoped_key(&account_path, &storage_path)?;
        let entry = files
            .get(&storage_path)
            .with_context(|| "a database blob is missing from DATA_DIR")?;
        ensure!(
            entry.size == u64::try_from(stored_size)?,
            "a database blob size does not match DATA_DIR"
        );
        if encoding == "plain-v1" {
            let content_hash = content_hash.context("plain blob is missing its content hash")?;
            ensure!(
                valid_blake3(&content_hash) && entry.blake3 == content_hash.to_ascii_lowercase(),
                "a database blob hash does not match DATA_DIR"
            );
        }
    }

    let mut commits = connection.prepare(
        "SELECT u.id, u.commit_state, u.commit_staged_key, u.commit_final_key, \
                u.commit_expected_size, u.commit_expected_blake3, u.commit_account_path, \
                a.storage_path, u.commit_blob_id \
         FROM uploads u JOIN accounts a ON a.id = u.account_id \
         WHERE u.commit_state IN ('commit_started','finalizing') ORDER BY u.id",
    )?;
    let rows = commits.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, Option<String>>(8)?,
        ))
    })?;
    for row in rows {
        let (id, state, staged, final_key, size, hash, commit_account, account, blob_id) = row?;
        let upload_id = Uuid::parse_str(&id).context("active upload ID is invalid")?;
        ensure!(
            commit_account.as_deref() == Some(account.as_str()),
            "upload commit account path conflicts with its account"
        );
        ensure!(
            blob_id
                .as_deref()
                .is_some_and(|value| Uuid::parse_str(value).is_ok()),
            "upload commit blob ID is missing or invalid"
        );
        let size = u64::try_from(size.context("upload commit size is missing")?)?;
        let hash = hash.context("upload commit hash is missing")?;
        ensure!(valid_blake3(&hash), "upload commit hash is invalid");
        let final_key = final_key.context("upload commit final key is missing")?;
        ensure_scoped_key(&account, &final_key)?;
        if state == "commit_started" {
            ensure!(
                staged.is_some(),
                "commit-started upload has no staged object key"
            );
        }
        if let Some(path) = staged.as_deref() {
            ensure_commit_stage_key(&account, upload_id, path)?;
        }
        let stage_matches = staged
            .as_deref()
            .and_then(|path| files.get(path))
            .is_some_and(|entry| entry.size == size && entry.blake3 == hash);
        let final_matches = files
            .get(&final_key)
            .is_some_and(|entry| entry.size == size && entry.blake3 == hash);
        if let Some(path) = staged.as_deref() {
            if let Some(entry) = files.get(path) {
                ensure!(
                    entry.size == size && entry.blake3 == hash,
                    "staged upload object is corrupt"
                );
            }
        }
        if let Some(entry) = files.get(&final_key) {
            ensure!(
                entry.size == size && entry.blake3 == hash,
                "finalizing upload object is corrupt"
            );
        }
        if state == "finalizing" {
            ensure!(
                stage_matches || final_matches,
                "finalizing upload has no recoverable staged or final object"
            );
        }
    }
    Ok(())
}

fn validate_account_storage_paths(connection: &Connection) -> anyhow::Result<()> {
    let mut statement = connection.prepare("SELECT storage_path FROM accounts ORDER BY id")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut paths = Vec::new();
    for path in rows {
        let path = path?;
        validate_storage_key(&path)?;
        ensure!(
            Path::new(&path).components().next() != Some(Component::Normal("uploads".as_ref())),
            "an account storage path uses the reserved uploads directory"
        );
        paths.push(path);
    }
    for (index, path) in paths.iter().enumerate() {
        for other in paths.iter().skip(index + 1) {
            let path = Path::new(path);
            let other = Path::new(other);
            let overlaps = path
                .strip_prefix(other)
                .is_ok_and(|relative| !relative.as_os_str().is_empty())
                || other
                    .strip_prefix(path)
                    .is_ok_and(|relative| !relative.as_os_str().is_empty());
            ensure!(!overlaps, "account storage paths overlap");
        }
    }
    Ok(())
}

fn validate_upload_manifests(connection: &Connection, files: &FileIndex) -> anyhow::Result<()> {
    let mut expected_parts = BTreeMap::new();
    let mut uploads = connection.prepare(
        "SELECT u.id, u.request, u.source_resource_id, u.dedup_token, \
                a.source_asset_id, a.media_kind \
         FROM uploads u JOIN assets a ON a.id = u.asset_id ORDER BY u.id",
    )?;
    let rows = uploads.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    for row in rows {
        let (upload_id, request, source_resource_id, dedup_token, source_asset_id, media_kind) =
            row?;
        ensure!(Uuid::parse_str(&upload_id).is_ok(), "upload ID is invalid");
        let request: CreateUploadRequest =
            serde_json::from_str(&request).context("an upload request is invalid")?;
        ensure!(
            !request.source_asset_id.is_empty()
                && !request.source_resource_id.is_empty()
                && !request.filename.is_empty()
                && !request.mime_type.is_empty()
                && valid_blake3(&request.content_blake3)
                && !request.parts.is_empty()
                && request.source_asset_id == source_asset_id
                && request.source_resource_id == source_resource_id
                && request.content_blake3 == dedup_token
                && request.media_kind.as_str() == media_kind,
            "an upload request conflicts with its database metadata"
        );
        let mut total = 0_u64;
        for (position, part) in request.parts.into_iter().enumerate() {
            ensure!(
                usize::try_from(part.index)? == position
                    && (part.size != 0 || request.content_size == 0)
                    && valid_blake3(&part.blake3),
                "an upload part manifest is invalid"
            );
            total = total
                .checked_add(part.size)
                .context("upload part size overflow")?;
            ensure!(
                expected_parts
                    .insert(
                        (upload_id.clone(), i64::from(part.index)),
                        (part.size, part.blake3)
                    )
                    .is_none(),
                "an upload contains duplicate part indexes"
            );
        }
        ensure!(
            total == request.content_size,
            "upload parts do not match the requested content size"
        );
    }

    let mut durable_parts = BTreeSet::new();
    let mut parts = connection.prepare(
        "SELECT upload_id, part_index, expected_size, expected_blake3, received_size, received_at \
         FROM upload_parts ORDER BY upload_id, part_index",
    )?;
    let rows = parts.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    for row in rows {
        let (upload_id, index, size, hash, received_size, received_at) = row?;
        let expected = expected_parts
            .remove(&(upload_id.clone(), index))
            .context("an upload part is absent from its persisted request")?;
        let size = u64::try_from(size)?;
        ensure!(
            expected == (size, hash.clone()),
            "an upload part conflicts with its persisted request"
        );
        if received_at.is_some() {
            ensure!(
                received_size.and_then(|value| u64::try_from(value).ok()) == Some(size),
                "a received upload part has invalid durable size metadata"
            );
            let index = u32::try_from(index)?;
            let path = format!("uploads/{upload_id}/{index:08}.part");
            validate_expected_file(files, &path, size, &hash, "upload part")?;
            durable_parts.insert(path);
        } else {
            ensure!(
                received_size.is_none(),
                "an unreceived upload part has durable size metadata"
            );
        }
    }
    ensure!(
        expected_parts.is_empty(),
        "an upload request part is absent from SQLite"
    );
    let disk_parts: BTreeSet<String> = files
        .keys()
        .filter(|path| is_upload_part_path(path))
        .cloned()
        .collect();
    ensure!(
        disk_parts == durable_parts,
        "DATA_DIR upload parts do not match durable SQLite state"
    );
    Ok(())
}

fn validate_expected_file(
    files: &FileIndex,
    path: &str,
    size: u64,
    hash: &str,
    kind: &str,
) -> anyhow::Result<()> {
    ensure!(valid_blake3(hash), "{kind} hash is invalid");
    let entry = files
        .get(path)
        .with_context(|| format!("{kind} is missing from DATA_DIR"))?;
    ensure!(
        entry.size == size && entry.blake3 == hash.to_ascii_lowercase(),
        "{kind} hash or size does not match DATA_DIR"
    );
    Ok(())
}

fn ensure_scoped_key(account: &str, object: &str) -> anyhow::Result<()> {
    validate_storage_key(account)?;
    validate_storage_key(object)?;
    let relative = Path::new(object)
        .strip_prefix(account)
        .context("blob is outside its account storage path")?;
    ensure!(
        !relative.as_os_str().is_empty(),
        "blob path must be below its account storage path"
    );
    Ok(())
}

fn ensure_commit_stage_key(account: &str, upload_id: Uuid, staged: &str) -> anyhow::Result<()> {
    validate_storage_key(staged)?;
    let staged = Path::new(staged);
    ensure!(
        staged.parent() == Some(Path::new(account).join("staging").as_path()),
        "staged upload object is outside its account staging directory"
    );
    let name = staged
        .file_name()
        .and_then(|value| value.to_str())
        .context("staged upload object name is invalid")?;
    let nonce = name
        .strip_prefix(&format!("commit-{upload_id}-"))
        .and_then(|value| value.strip_suffix(".stage"))
        .context("staged upload object name does not match its upload")?;
    ensure!(
        Uuid::parse_str(nonce).is_ok(),
        "staged upload object nonce is invalid"
    );
    Ok(())
}

fn validate_storage_key(value: &str) -> anyhow::Result<()> {
    validate_relative_key(value)?;
    ensure!(
        value.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.starts_with('.')
                && component.trim() == component
        }),
        "database storage key is invalid"
    );
    Ok(())
}

fn valid_blake3(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_upload_part_path(path: &str) -> bool {
    path.starts_with("uploads/") && path.ends_with(".part")
}

fn online_backup(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let destination_path = destination.to_path_buf();
    ensure_regular_file(source, "SQLite source")?;
    let source = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("open SQLite online-backup source")?;
    source.busy_timeout(Duration::from_secs(5))?;
    let mut destination = Connection::open_with_flags(
        destination,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("open SQLite online-backup destination")?;
    {
        let backup =
            Backup::new(&source, &mut destination).context("start SQLite online backup")?;
        backup
            .run_to_completion(128, Duration::from_millis(10), None)
            .context("copy SQLite pages including WAL state")?;
    }
    destination.execute_batch("PRAGMA journal_mode=DELETE;")?;
    drop(destination);
    sync_file_and_parent(&destination_path)?;
    Ok(())
}

fn copy_data_tree(source_root: &Path, destination_root: &Path) -> anyhow::Result<FileIndex> {
    let rooted = RootedFs::new(source_root).context("open DATA_DIR without following links")?;
    let paths = walk_files(&rooted)?;
    let mut index = BTreeMap::new();
    for path in paths {
        let relative = Path::new(&path);
        let destination = safe_join(destination_root, relative)?;
        create_private_parents(destination_root, relative)?;
        let mut input = rooted
            .open_read_std(relative)
            .with_context(|| "open DATA_DIR file without following links")?;
        let mut output = create_private_file(&destination)?;
        let (size, hash) = copy_and_hash(&mut input, &mut output)?;
        output.sync_all()?;
        index.insert(
            path.clone(),
            FileManifest {
                path,
                size,
                blake3: hash,
            },
        );
    }
    Ok(index)
}

fn copy_manifest_files(backup: &VerifiedBackup, destination_root: &Path) -> anyhow::Result<()> {
    let source_root = backup.root.join(FILES_DIRECTORY);
    let rooted =
        RootedFs::new(&source_root).context("open backup files without following links")?;
    for entry in &backup.manifest.files {
        let relative = Path::new(&entry.path);
        validate_relative_path(relative)?;
        create_private_parents(destination_root, relative)?;
        let destination = safe_join(destination_root, relative)?;
        let mut input = rooted
            .open_read_std(relative)
            .context("open backup data file")?;
        let mut output = create_private_file(&destination)?;
        let (size, hash) = copy_and_hash(&mut input, &mut output)?;
        ensure!(
            size == entry.size && hash == entry.blake3,
            "backup data changed while restoring"
        );
        output.sync_all()?;
    }
    Ok(())
}

fn copy_backup_database(backup: &VerifiedBackup, destination: &Path) -> anyhow::Result<()> {
    let rooted = RootedFs::new(&backup.root).context("open backup without following links")?;
    let mut input = rooted.open_read_std(Path::new(DATABASE_FILE))?;
    let mut output = create_private_file(destination)?;
    let (size, hash) = copy_and_hash(&mut input, &mut output)?;
    ensure!(
        size == backup.manifest.database.size && hash == backup.manifest.database.blake3,
        "backup database changed while restoring"
    );
    output.sync_all()?;
    Ok(())
}

fn index_data_tree(root: &Path) -> anyhow::Result<FileIndex> {
    let rooted = RootedFs::new(root).context("open data tree without following links")?;
    let paths = walk_files(&rooted)?;
    let mut index = BTreeMap::new();
    for path in paths {
        let (size, hash) = hash_rooted(&rooted, Path::new(&path))?;
        index.insert(
            path.clone(),
            FileManifest {
                path,
                size,
                blake3: hash,
            },
        );
    }
    Ok(index)
}

fn walk_files(rooted: &RootedFs) -> anyhow::Result<BTreeSet<String>> {
    fn visit(
        rooted: &RootedFs,
        directory: &Path,
        files: &mut BTreeSet<String>,
    ) -> anyhow::Result<()> {
        let mut entries = rooted
            .list_entries(directory)
            .context("walk rooted file tree")?;
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, kind) in entries {
            let name = name.to_str().context("backup paths must be valid UTF-8")?;
            let path = if directory.as_os_str().is_empty() {
                PathBuf::from(name)
            } else {
                directory.join(name)
            };
            validate_relative_path(&path)?;
            match kind {
                RootEntryKind::Directory => visit(rooted, &path, files)?,
                RootEntryKind::RegularFile => {
                    files.insert(
                        path.to_str()
                            .context("backup paths must be valid UTF-8")?
                            .to_owned(),
                    );
                }
            }
        }
        Ok(())
    }

    let mut files = BTreeSet::new();
    visit(rooted, Path::new(""), &mut files)?;
    Ok(files)
}

fn validate_manifest_files(manifest: &BackupManifest, actual: &FileIndex) -> anyhow::Result<()> {
    let expected: FileIndex = manifest
        .files
        .iter()
        .cloned()
        .map(|entry| (entry.path.clone(), entry))
        .collect();
    ensure!(
        expected.len() == manifest.files.len(),
        "backup manifest contains duplicate file paths"
    );
    ensure!(
        expected == *actual,
        "backup file hashes do not match manifest"
    );
    Ok(())
}

fn ensure_manifest_is_canonical(manifest: &BackupManifest) -> anyhow::Result<()> {
    ensure!(
        manifest.database.size > 0 && valid_blake3(&manifest.database.blake3),
        "backup database manifest is invalid"
    );
    let mut previous: Option<&str> = None;
    for entry in &manifest.files {
        validate_relative_key(&entry.path)?;
        ensure!(valid_blake3(&entry.blake3), "backup file hash is invalid");
        if let Some(previous) = previous {
            ensure!(
                previous < entry.path.as_str(),
                "backup file list is not canonical"
            );
        }
        previous = Some(&entry.path);
    }
    let actual_counts: BTreeSet<&str> = manifest.record_counts.keys().map(String::as_str).collect();
    let expected_counts: BTreeSet<&str> = COUNTED_TABLES.into_iter().collect();
    ensure!(
        actual_counts == expected_counts,
        "backup record count set is invalid"
    );
    Ok(())
}

fn read_manifest(rooted: &RootedFs) -> anyhow::Result<BackupManifest> {
    let mut file = rooted
        .open_read_std(Path::new(MANIFEST_FILE))
        .context("open backup manifest")?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        u64::try_from(bytes.len())? <= MANIFEST_LIMIT,
        "backup manifest is too large"
    );
    serde_json::from_slice(&bytes).context("parse backup manifest")
}

fn write_manifest(path: &Path, manifest: &BackupManifest) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    ensure!(
        u64::try_from(bytes.len())? <= MANIFEST_LIMIT,
        "backup manifest is too large"
    );
    let mut file = create_private_file(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn hash_rooted(rooted: &RootedFs, relative: &Path) -> anyhow::Result<(u64, String)> {
    let mut file = rooted.open_read_std(relative)?;
    hash_reader(&mut file)
}

fn hash_path(path: &Path) -> anyhow::Result<(u64, String)> {
    ensure_regular_file(path, "file to hash")?;
    hash_reader(&mut File::open(path)?)
}

fn hash_reader(reader: &mut impl Read) -> anyhow::Result<(u64, String)> {
    let mut hasher = blake3::Hasher::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(read)?)
            .context("file is too large")?;
        hasher.update(&buffer[..read]);
    }
    Ok((total, hasher.finalize().to_hex().to_string()))
}

fn copy_and_hash(reader: &mut impl Read, writer: &mut impl Write) -> anyhow::Result<(u64, String)> {
    let mut hasher = blake3::Hasher::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        total = total
            .checked_add(u64::try_from(read)?)
            .context("file is too large")?;
        hasher.update(&buffer[..read]);
    }
    Ok((total, hasher.finalize().to_hex().to_string()))
}

fn live_paths(config: &Config) -> anyhow::Result<(PathBuf, PathBuf)> {
    let database = existing_anchored_file(
        &sqlite_database_path(&config.database_url)?,
        "SQLite database",
    )?;
    let data_dir = existing_anchored_directory(&config.data_dir, "DATA_DIR")?;
    ensure!(
        !database.starts_with(&data_dir),
        "SQLite database must be outside DATA_DIR for joint atomic restore"
    );
    Ok((database, data_dir))
}

fn live_paths_for_restore(config: &Config) -> anyhow::Result<(PathBuf, PathBuf)> {
    let database = anchored_destination(
        &sqlite_database_path(&config.database_url)?,
        "SQLite database",
    )?;
    let data_dir = anchored_destination(&config.data_dir, "DATA_DIR")?;
    ensure!(
        !database.starts_with(&data_dir),
        "SQLite database must be outside DATA_DIR for joint atomic restore"
    );
    Ok((database, data_dir))
}

fn existing_anchored_file(path: &Path, label: &str) -> anyhow::Result<PathBuf> {
    let anchored = anchored_destination(path, label)?;
    ensure_regular_file(&anchored, label)?;
    Ok(anchored)
}

fn existing_anchored_directory(path: &Path, label: &str) -> anyhow::Result<PathBuf> {
    let anchored = anchored_destination(path, label)?;
    let metadata = fs::symlink_metadata(&anchored).with_context(|| format!("open {label}"))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "{label} must not be a symbolic link"
    );
    ensure!(metadata.is_dir(), "{label} is not a directory");
    let canonical = fs::canonicalize(&anchored).with_context(|| format!("resolve {label}"))?;
    ensure!(canonical == anchored, "{label} path is not canonical");
    Ok(anchored)
}

fn anchored_destination(path: &Path, label: &str) -> anyhow::Result<PathBuf> {
    let absolute = clean_absolute_path(path, label)?;
    let name = absolute
        .file_name()
        .with_context(|| format!("{label} must name an entry"))?;
    let parent = absolute
        .parent()
        .with_context(|| format!("{label} must have a parent directory"))?;
    reject_symlink_directory_chain(parent, label)?;
    let canonical = fs::canonicalize(parent).with_context(|| format!("resolve {label} parent"))?;
    ensure!(canonical == parent, "{label} parent path is not canonical");
    Ok(canonical.join(name))
}

fn clean_absolute_path(path: &Path, label: &str) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .with_context(|| format!("resolve {label} working directory"))?
            .join(path)
    };
    let mut clean = PathBuf::from("/");
    for component in absolute.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(value) => clean.push(value),
            Component::ParentDir => anyhow::bail!("{label} contains a path escape"),
            Component::Prefix(_) => anyhow::bail!("{label} has an unsupported path prefix"),
        }
    }
    Ok(clean)
}

fn reject_symlink_directory_chain(path: &Path, label: &str) -> anyhow::Result<()> {
    let mut current = PathBuf::from("/");
    for component in path.components() {
        let Component::Normal(value) = component else {
            continue;
        };
        current.push(value);
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("inspect {label} parent directory"))?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "{label} parent contains a symbolic link or non-directory"
        );
    }
    Ok(())
}

fn absent_anchored_target(path: &Path, label: &str) -> anyhow::Result<PathBuf> {
    let target = anchored_destination(path, label)?;
    match fs::symlink_metadata(&target) {
        Ok(_) => anyhow::bail!("{label} already exists; refusing to overwrite"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(target),
        Err(error) => Err(error).with_context(|| format!("inspect {label}")),
    }
}

fn ensure_regular_file(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("inspect {label}"))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "{label} must not be a symbolic link"
    );
    ensure!(metadata.is_file(), "{label} is not a regular file");
    Ok(())
}

fn reject_symlink_or_special_destination(path: &Path, directory: bool) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                !metadata.file_type().is_symlink(),
                "restore destination is a symbolic link"
            );
            ensure!(
                if directory {
                    metadata.is_dir()
                } else {
                    metadata.is_file()
                },
                "restore destination has the wrong file type"
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect restore destination"),
    }
    Ok(())
}

fn validate_relative_key(value: &str) -> anyhow::Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 4096
            && value.trim() == value
            && !value.contains(['\\', ':', '\0'])
            && !value.chars().any(char::is_control),
        "backup path is invalid"
    );
    validate_relative_path(Path::new(value))
}

fn validate_relative_path(path: &Path) -> anyhow::Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "backup path contains an escape or non-normal component"
    );
    Ok(())
}

fn safe_join(root: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
    validate_relative_path(relative)?;
    Ok(root.join(relative))
}

fn create_private_directory(path: &Path) -> anyhow::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)?;
    Ok(())
}

fn create_private_parents(root: &Path, relative: &Path) -> anyhow::Result<()> {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut current = root.to_path_buf();
    for component in parent.components() {
        let Component::Normal(component) = component else {
            anyhow::bail!("backup path contains a non-normal component");
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "backup staging path is unsafe"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_private_directory(&current)?
            }
            Err(error) => return Err(error).context("inspect backup staging directory"),
        }
    }
    Ok(())
}

fn create_private_file(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).context("create private backup file")
}

fn adjacent_unique_path(
    destination: &Path,
    kind: &str,
    directory: bool,
) -> anyhow::Result<PathBuf> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .context("restore destination name must be UTF-8")?;
    for _ in 0..32 {
        let candidate = parent.join(format!(".{name}.{kind}-{}", Uuid::new_v4()));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "cannot allocate adjacent {} staging path",
        if directory { "directory" } else { "file" }
    )
}

fn prepare_database_destination(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        ensure_regular_file(path, "restore database destination")?;
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(2))?;
        let (busy, _, _): (i64, i64, i64) =
            connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        ensure!(busy == 0, "restore database is busy; stop all writers");
        connection.pragma_update(None, "locking_mode", "EXCLUSIVE")?;
        connection
            .execute_batch("BEGIN EXCLUSIVE; ROLLBACK;")
            .context("restore database is in use")?;
        drop(connection);
    }
    remove_sqlite_sidecars(path)?;
    Ok(())
}

fn rollback_install(
    database: &Path,
    data_dir: &Path,
    old_database: &Path,
    old_data: &Path,
    had_database: bool,
    had_data: bool,
) -> anyhow::Result<()> {
    let failed_database = adjacent_unique_path(database, "failed", false)?;
    let failed_data = adjacent_unique_path(data_dir, "failed", true)?;
    if database.exists() {
        fs::rename(database, &failed_database).context("move failed restored database aside")?;
    }
    if data_dir.exists() {
        fs::rename(data_dir, &failed_data).context("move failed restored DATA_DIR aside")?;
    }
    if had_database {
        fs::rename(old_database, database).context("roll back original database")?;
    }
    if had_data {
        fs::rename(old_data, data_dir).context("roll back original DATA_DIR")?;
    }
    if failed_database.exists() {
        fs::remove_file(&failed_database)?;
    }
    if failed_data.exists() {
        fs::remove_dir_all(&failed_data)?;
    }
    remove_sqlite_sidecars(database)?;
    sync_parent(database)?;
    sync_parent(data_dir)?;
    Ok(())
}

fn remove_sqlite_sidecars(path: &Path) -> anyhow::Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        match fs::symlink_metadata(&sidecar) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "SQLite sidecar is not a regular file"
                );
                fs::remove_file(&sidecar)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect SQLite sidecar"),
        }
    }
    Ok(())
}

fn database_write_rollback_probe(path: &Path) -> anyhow::Result<()> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(2))?;
    connection.execute_batch(
        "BEGIN IMMEDIATE;\
         CREATE TABLE __photo_backup_doctor_probe(value INTEGER NOT NULL);\
         INSERT INTO __photo_backup_doctor_probe(value) VALUES(1);\
         SELECT value FROM __photo_backup_doctor_probe;\
         ROLLBACK;",
    )?;
    let exists: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name='__photo_backup_doctor_probe'",
        [],
        |row| row.get(0),
    )?;
    ensure!(exists == 0, "database rollback probe left persistent state");
    Ok(())
}

fn storage_write_cleanup_probe(data_dir: &Path) -> anyhow::Result<()> {
    let rooted = RootedFs::new(data_dir)?;
    let relative = PathBuf::from(format!(".photo-backup-doctor-{}", Uuid::new_v4()));
    let mut file = rooted.create_new_std(&relative)?;
    let probe_result = (|| -> anyhow::Result<()> {
        file.write_all(b"photo-backup-doctor-v1")?;
        file.sync_all()?;
        drop(file);
        let mut file = rooted.open_read_std(&relative)?;
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        ensure!(
            contents == b"photo-backup-doctor-v1",
            "storage read/write probe failed"
        );
        Ok(())
    })();
    let cleanup_result = rooted.remove_file(&relative);
    probe_result?;
    ensure!(cleanup_result?, "storage probe cleanup failed");
    Ok(())
}

fn sync_file_and_parent(path: &Path) -> anyhow::Result<()> {
    File::open(path)?.sync_all()?;
    sync_parent(path)
}

fn sync_parent(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()?;
    Ok(())
}

fn sync_tree(root: &Path) -> anyhow::Result<()> {
    fn visit(path: &Path) -> anyhow::Result<()> {
        let mut directories = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.file_type()?;
            ensure!(!metadata.is_symlink(), "refusing to sync a symbolic link");
            if metadata.is_dir() {
                visit(&entry.path())?;
                directories.push(entry.path());
            } else {
                ensure!(metadata.is_file(), "refusing to sync a special file");
                File::open(entry.path())?.sync_all()?;
            }
        }
        for directory in directories {
            File::open(directory)?.sync_all()?;
        }
        File::open(path)?.sync_all()?;
        Ok(())
    }
    visit(root)
}

fn summary(manifest: &BackupManifest) -> BackupSummary {
    BackupSummary {
        files: manifest.files.len(),
        bytes: manifest.files.iter().map(|entry| entry.size).sum(),
        schema_version: manifest.schema_version,
    }
}

struct PendingFile {
    path: PathBuf,
    committed: bool,
}

impl PendingFile {
    fn create_adjacent(destination: &Path, kind: &str) -> anyhow::Result<(Self, PathBuf)> {
        let path = adjacent_unique_path(destination, kind, false)?;
        Ok((
            Self {
                path: path.clone(),
                committed: false,
            },
            path,
        ))
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
            let _ = remove_sqlite_sidecars(&self.path);
        }
    }
}

struct PendingDirectory {
    path: PathBuf,
    committed: bool,
}

impl PendingDirectory {
    fn create_adjacent(destination: &Path, kind: &str) -> anyhow::Result<(Self, PathBuf)> {
        let path = adjacent_unique_path(destination, kind, true)?;
        create_private_directory(&path)?;
        Ok((
            Self {
                path: path.clone(),
                committed: false,
            },
            path,
        ))
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingDirectory {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::{
        os::unix::fs::symlink,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        },
        thread,
    };

    use sha2::{Digest, Sha256};
    use sqlx::Row;

    use super::*;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("photo-backup-joint-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn database(&self) -> PathBuf {
            self.0.join("db").join("app.sqlite3")
        }

        fn data(&self) -> PathBuf {
            self.0.join("data")
        }

        fn backup(&self) -> PathBuf {
            self.0.join("snapshot")
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    async fn setup() -> (TestRoot, Config, String, Vec<u8>, String, Vec<u8>) {
        let root = TestRoot::new();
        fs::create_dir_all(root.database().parent().unwrap()).unwrap();
        fs::create_dir_all(root.data().join("uploads")).unwrap();
        fs::create_dir_all(root.data().join("blobs")).unwrap();
        let database_url = format!("sqlite://{}", root.database().display());
        let pool = crate::database::connect(&database_url).await.unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool.close().await;

        let account_id = Uuid::new_v4().to_string();
        let content = b"joint-backup-blob".to_vec();
        let hash = blake3::hash(&content).to_hex().to_string();
        let object = format!("blobs/{account_id}/objects/{}/{hash}", &hash[..2]);
        let object_path = root.data().join(&object);
        fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        fs::write(&object_path, &content).unwrap();

        let connection = Connection::open(root.database()).unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        connection
            .execute(
                "INSERT INTO accounts(\
                   id, created_at, display_name, storage_path, quota_bytes, enabled, username\
                 ) VALUES(?, datetime('now'), 'Backup Owner', ?, 1000000, 1, 'backup-owner')",
                rusqlite::params![account_id, format!("blobs/{account_id}")],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO blobs(\
                   id, account_id, dedup_token, plaintext_size, stored_size, storage_path,\
                   part_manifest, content_blake3, storage_encoding, created_at\
                 ) VALUES(?, ?, ?, ?, ?, ?, '[]', ?, 'plain-v1', datetime('now'))",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    account_id,
                    hash,
                    content.len() as i64,
                    content.len() as i64,
                    object,
                    hash
                ],
            )
            .unwrap();
        let device_id = Uuid::new_v4().to_string();
        let asset_id = Uuid::new_v4().to_string();
        let upload_id = Uuid::new_v4().to_string();
        let part_content = b"durable-unfinished-upload-part".to_vec();
        let part_hash = blake3::hash(&part_content).to_hex().to_string();
        let part = format!("uploads/{upload_id}/00000000.part");
        let upload_request = serde_json::to_string(&CreateUploadRequest {
            source_asset_id: "unfinished-asset".to_owned(),
            source_resource_id: "unfinished-resource".to_owned(),
            media_kind: photo_backup_protocol::MediaKind::Photo,
            role: "original".to_owned(),
            filename: "unfinished.jpg".to_owned(),
            mime_type: "image/jpeg".to_owned(),
            source_created_at_ms: 1,
            content_size: part_content.len() as u64,
            content_blake3: part_hash.clone(),
            metadata: None,
            parts: vec![photo_backup_protocol::UploadPartSpec {
                index: 0,
                size: part_content.len() as u64,
                blake3: part_hash.clone(),
            }],
        })
        .unwrap();
        fs::create_dir_all(root.data().join(&part).parent().unwrap()).unwrap();
        fs::write(root.data().join(&part), &part_content).unwrap();
        connection
            .execute(
                "INSERT INTO devices(\
                   id, account_id, name, platform, token_hash, created_at, last_seen_at\
                 ) VALUES(?, ?, 'Backup Phone', 'test', ?, datetime('now'), datetime('now'))",
                rusqlite::params![device_id, account_id, vec![7_u8; 32]],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO assets(\
                   id, account_id, device_id, source_asset_id, media_kind, source_created_at_ms,\
                   created_at, updated_at\
                 ) VALUES(?, ?, ?, 'unfinished-asset', 'photo', 1, datetime('now'), datetime('now'))",
                rusqlite::params![asset_id, account_id, device_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO uploads(\
                   id, account_id, device_id, asset_id, source_resource_id, dedup_token, request,\
                   state, created_at, updated_at\
                 ) VALUES(?, ?, ?, ?, 'unfinished-resource', ?, ?, 'uploading',\
                          datetime('now'), datetime('now'))",
                rusqlite::params![
                    upload_id,
                    account_id,
                    device_id,
                    asset_id,
                    part_hash,
                    upload_request
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO upload_parts(\
                   upload_id, part_index, expected_size, expected_blake3, received_size, received_at\
                 ) VALUES(?, 0, ?, ?, ?, datetime('now'))",
                rusqlite::params![
                    upload_id,
                    part_content.len() as i64,
                    part_hash,
                    part_content.len() as i64
                ],
            )
            .unwrap();
        drop(connection);

        let config = Config {
            database_url,
            data_dir: root.data(),
            bind: "127.0.0.1:0".parse().unwrap(),
            admin_username: "backup-admin".to_owned(),
            admin_password: "do-not-put-this-password-in-manifest".to_owned(),
            max_part_bytes: 1024 * 1024,
            metrics_token: Some("do-not-put-this-token-in-manifest".to_owned()),
            require_https: false,
            development: true,
            admin_session_idle_seconds: 1_800,
            admin_session_absolute_seconds: 43_200,
            trusted_proxy_cidrs: Vec::new(),
        };
        (root, config, object, content, part, part_content)
    }

    #[tokio::test]
    async fn create_verify_doctor_are_joint_non_overwriting_and_secret_free() {
        let (root, config, object, content, part, part_content) = setup().await;
        let session_token = "do-not-put-this-session-token-in-backup-output";
        let csrf_token = "do-not-put-this-csrf-token-in-backup-output";
        let api_key = "do-not-put-this-api-key-in-backup-output";
        let password_hash = crate::password::hash_password(config.admin_password.clone())
            .await
            .unwrap();
        let connection = Connection::open(root.database()).unwrap();
        let auth_user_id = Uuid::new_v4().to_string();
        connection
            .execute(
                "INSERT INTO auth_users(\
                   id, username, password_hash, role, active, session_version, created_at, updated_at\
                 ) VALUES(?, 'backup-admin', ?, 'admin', 1, 1, unixepoch(), unixepoch())",
                rusqlite::params![auth_user_id, password_hash],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO auth_sessions(\
                   id, user_id, token_hash, csrf_hash, user_session_version, created_at,\
                   last_seen_at, idle_expires_at, absolute_expires_at\
                 ) VALUES(?, ?, ?, ?, 1, unixepoch(), unixepoch(), unixepoch()+60, unixepoch()+120)",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    auth_user_id,
                    Sha256::digest(session_token.as_bytes()).to_vec(),
                    Sha256::digest(csrf_token.as_bytes()).to_vec()
                ],
            )
            .unwrap();
        let account_id: String = connection
            .query_row("SELECT id FROM accounts LIMIT 1", [], |row| row.get(0))
            .unwrap();
        let device_id: String = connection
            .query_row("SELECT id FROM devices LIMIT 1", [], |row| row.get(0))
            .unwrap();
        connection
            .execute(
                "INSERT INTO api_keys(\
                   id, account_id, device_id, name, prefix, token_hash, created_at\
                 ) VALUES(?, ?, ?, 'backup-key', 'photo_key', ?, datetime('now'))",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    account_id,
                    device_id,
                    Sha256::digest(api_key.as_bytes()).to_vec()
                ],
            )
            .unwrap();
        drop(connection);
        let summary = create(&config, &root.backup()).unwrap();
        assert_eq!(summary.files, 2);
        verify(&config, &root.backup()).unwrap();
        assert!(create(&config, &root.backup()).is_err());
        let doctor = doctor(&config).unwrap();
        assert_eq!(doctor.status, "ok");
        assert_eq!(doctor.blobs, 1);
        assert_eq!(doctor.uploads, 1);
        assert_eq!(
            fs::read(root.backup().join("files").join(object)).unwrap(),
            content
        );
        assert_eq!(
            fs::read(root.backup().join("files").join(&part)).unwrap(),
            part_content
        );
        let manifest = fs::read_to_string(root.backup().join(MANIFEST_FILE)).unwrap();
        assert!(!manifest.contains(&config.admin_password));
        assert!(!manifest.contains(config.metrics_token.as_deref().unwrap()));
        assert!(!manifest.contains(&config.database_url));
        let parsed_manifest: BackupManifest = serde_json::from_str(&manifest).unwrap();
        assert_eq!(parsed_manifest.database.file, DATABASE_FILE);
        assert!(parsed_manifest
            .files
            .iter()
            .all(|entry| !Path::new(&entry.path).is_absolute()));
        let backup_files = walk_files(&RootedFs::new(&root.backup()).unwrap()).unwrap();
        for path in backup_files {
            let bytes = fs::read(root.backup().join(path)).unwrap();
            for secret in [
                config.admin_password.as_bytes(),
                config.metrics_token.as_deref().unwrap().as_bytes(),
                config.database_url.as_bytes(),
                session_token.as_bytes(),
                csrf_token.as_bytes(),
                api_key.as_bytes(),
            ] {
                assert!(!bytes.windows(secret.len()).any(|window| window == secret));
            }
        }
        assert!(fs::read_dir(root.data()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("doctor")));
    }

    #[tokio::test]
    async fn verification_rejects_tampering_missing_files_and_wrong_product() {
        let (root, config, object, _, _, _) = setup().await;
        create(&config, &root.backup()).unwrap();
        fs::write(root.backup().join("files").join(&object), b"tampered").unwrap();
        assert!(verify(&config, &root.backup()).is_err());

        fs::remove_dir_all(root.backup()).unwrap();
        create(&config, &root.backup()).unwrap();
        fs::remove_file(root.backup().join("files").join(&object)).unwrap();
        assert!(verify(&config, &root.backup()).is_err());

        fs::remove_dir_all(root.backup()).unwrap();
        create(&config, &root.backup()).unwrap();
        let manifest_path = root.backup().join(MANIFEST_FILE);
        let mut manifest: BackupManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest.application = "another-product".to_owned();
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        assert!(verify(&config, &root.backup()).is_err());

        fs::remove_dir_all(root.backup()).unwrap();
        create(&config, &root.backup()).unwrap();
        fs::remove_file(root.backup().join(DATABASE_FILE)).unwrap();
        assert!(verify(&config, &root.backup()).is_err());

        fs::remove_dir_all(root.backup()).unwrap();
        create(&config, &root.backup()).unwrap();
        let alias = root.0.join("backup-link");
        symlink(root.backup(), &alias).unwrap();
        assert!(verify(&config, &alias).is_err());
        fs::remove_file(alias).unwrap();
        let linked_root = root.0.join("linked-root");
        symlink(&root.0, &linked_root).unwrap();
        assert!(verify(&config, &linked_root.join("snapshot")).is_err());
        fs::remove_file(linked_root).unwrap();

        let manifest_path = root.backup().join(MANIFEST_FILE);
        let mut manifest: BackupManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest.files[0].path = "../escaped".to_owned();
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        assert!(verify(&config, &root.backup()).is_err());
    }

    #[tokio::test]
    async fn backup_rejects_symlinks_special_files_and_active_service_lock() {
        let (root, config, _, _, _, _) = setup().await;
        let outside = root.0.join("outside");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, root.data().join("unsafe-link")).unwrap();
        assert!(create(&config, &root.backup()).is_err());
        fs::remove_file(root.data().join("unsafe-link")).unwrap();

        let fifo = root.data().join("unsafe-fifo");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        assert!(create(&config, &root.backup()).is_err());
        fs::remove_file(&fifo).unwrap();

        let real_output_parent = root.0.join("real-output-parent");
        fs::create_dir(&real_output_parent).unwrap();
        let linked_output_parent = root.0.join("linked-output-parent");
        symlink(&real_output_parent, &linked_output_parent).unwrap();
        assert!(create(&config, &linked_output_parent.join("snapshot")).is_err());
        fs::remove_file(linked_output_parent).unwrap();

        create(&config, &root.backup()).unwrap();
        let service_lock = RuntimeLock::acquire(&config.database_url, &config.data_dir).unwrap();
        assert!(create(&config, &root.0.join("another-snapshot")).is_err());
        assert!(verify(&config, &root.backup()).is_err());
        assert!(restore(&config, &root.backup()).is_err());
        assert!(doctor(&config).is_err());
        drop(service_lock);
        verify(&config, &root.backup()).unwrap();
        doctor(&config).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sqlite_online_backup_is_consistent_while_wal_writer_commits() {
        let (root, config, _, _, _, _) = setup().await;
        let stop = Arc::new(AtomicBool::new(false));
        let writes = Arc::new(AtomicUsize::new(0));
        let database = root.database();
        let writer_stop = Arc::clone(&stop);
        let writer_writes = Arc::clone(&writes);
        let writer = thread::spawn(move || {
            let connection = Connection::open(database).unwrap();
            connection.busy_timeout(Duration::from_secs(2)).unwrap();
            connection
                .execute_batch("PRAGMA journal_mode=WAL;")
                .unwrap();
            while !writer_stop.load(Ordering::SeqCst) {
                let index = writer_writes.fetch_add(1, Ordering::SeqCst);
                let id = Uuid::new_v4().to_string();
                let result = connection.execute(
                    "INSERT INTO accounts(\
                       id, created_at, display_name, storage_path, quota_bytes, enabled, username\
                     ) VALUES(?, datetime('now'), 'WAL Writer', ?, 1, 1, ?)",
                    rusqlite::params![id, format!("wal/{index}"), format!("wal-{index}")],
                );
                if result.is_err() {
                    thread::yield_now();
                }
            }
        });
        while writes.load(Ordering::SeqCst) < 5 {
            tokio::task::yield_now().await;
        }
        create(&config, &root.backup()).unwrap();
        stop.store(true, Ordering::SeqCst);
        writer.join().unwrap();
        assert!(writes.load(Ordering::SeqCst) >= 5);
        verify(&config, &root.backup()).unwrap();
    }

    #[tokio::test]
    async fn every_install_and_post_verify_failure_rolls_back_both_targets() {
        let (root, config, object, original_content, _, _) = setup().await;
        create(&config, &root.backup()).unwrap();
        for failpoint in [
            RestoreFailpoint::AfterDatabaseInstall,
            RestoreFailpoint::AfterDataInstall,
            RestoreFailpoint::SidecarCleanup,
            RestoreFailpoint::ParentDirectorySync,
            RestoreFailpoint::PostInstallVerification,
        ] {
            let connection = Connection::open(root.database()).unwrap();
            connection
                .execute(
                    "UPDATE accounts SET display_name='Current State' WHERE username='backup-owner'",
                    [],
                )
                .unwrap();
            drop(connection);
            let current_content = format!("current-{failpoint:?}").into_bytes();
            fs::write(root.data().join(&object), &current_content).unwrap();
            let result = {
                let _lock = RuntimeLock::acquire(&config.database_url, &config.data_dir).unwrap();
                restore_locked(&config, &root.backup(), Some(failpoint))
            };
            assert!(result.is_err());
            assert_eq!(
                fs::read(root.data().join(&object)).unwrap(),
                current_content
            );
            let connection = Connection::open(root.database()).unwrap();
            let name: String = connection
                .query_row(
                    "SELECT display_name FROM accounts WHERE username='backup-owner'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(name, "Current State");
        }

        restore(&config, &root.backup()).unwrap();
        assert_eq!(
            fs::read(root.data().join(&object)).unwrap(),
            original_content
        );
        let connection = Connection::open(root.database()).unwrap();
        let name: String = connection
            .query_row(
                "SELECT display_name FROM accounts WHERE username='backup-owner'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Backup Owner");
    }

    #[tokio::test]
    async fn committed_restore_cleanup_failure_keeps_new_state_and_rollback_evidence() {
        let (root, config, object, backup_content, _, _) = setup().await;
        create(&config, &root.backup()).unwrap();
        let connection = Connection::open(root.database()).unwrap();
        connection
            .execute(
                "UPDATE accounts SET display_name='Pre-Restore State' WHERE username='backup-owner'",
                [],
            )
            .unwrap();
        drop(connection);
        let pre_restore_content = b"pre-restore-object".to_vec();
        fs::write(root.data().join(&object), &pre_restore_content).unwrap();

        let error = {
            let _lock = RuntimeLock::acquire(&config.database_url, &config.data_dir).unwrap();
            restore_locked(
                &config,
                &root.backup(),
                Some(RestoreFailpoint::OldGenerationCleanup),
            )
            .unwrap_err()
        };
        assert!(error
            .to_string()
            .contains("installed and verified successfully"));
        assert_eq!(fs::read(root.data().join(&object)).unwrap(), backup_content);
        let connection = Connection::open(root.database()).unwrap();
        let installed_name: String = connection
            .query_row(
                "SELECT display_name FROM accounts WHERE username='backup-owner'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(installed_name, "Backup Owner");
        drop(connection);

        let old_database = fs::read_dir(root.database().parent().unwrap())
            .unwrap()
            .map(Result::unwrap)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".app.sqlite3.rollback-")
            })
            .expect("old database rollback evidence")
            .path();
        let old_data = fs::read_dir(&root.0)
            .unwrap()
            .map(Result::unwrap)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".data.rollback-")
            })
            .expect("old DATA_DIR rollback evidence")
            .path();
        assert_eq!(
            fs::read(old_data.join(&object)).unwrap(),
            pre_restore_content
        );
        let connection = Connection::open(old_database).unwrap();
        let old_name: String = connection
            .query_row(
                "SELECT display_name FROM accounts WHERE username='backup-owner'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_name, "Pre-Restore State");
    }

    #[tokio::test]
    async fn restored_database_and_blob_are_readable_after_real_restart() {
        let (root, config, object, original_content, _, _) = setup().await;
        create(&config, &root.backup()).unwrap();
        fs::write(root.data().join(&object), b"changed-after-backup").unwrap();
        let restored_root = root.0.join("restored-instance");
        let restored_database = restored_root.join("db").join("app.sqlite3");
        fs::create_dir_all(restored_database.parent().unwrap()).unwrap();
        let restored_config = Config {
            database_url: format!("sqlite://{}", restored_database.display()),
            data_dir: restored_root.join("data"),
            ..config.clone()
        };
        restore(&restored_config, &root.backup()).unwrap();
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = restored_database.as_os_str().to_os_string();
            sidecar.push(suffix);
            assert!(!PathBuf::from(sidecar).exists());
        }
        verify(&config, &root.backup()).unwrap();

        let pool = crate::database::connect(&restored_config.database_url)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let row = sqlx::query(
            "SELECT a.storage_path AS account_path, b.storage_path \
             FROM blobs b JOIN accounts a ON a.id=b.account_id LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let storage = crate::storage::LocalStorage::new(restored_config.data_dir.clone())
            .await
            .unwrap();
        let mut blob = storage
            .open_blob(row.get("account_path"), row.get("storage_path"))
            .await
            .unwrap();
        let mut bytes = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut blob, &mut bytes)
            .await
            .unwrap();
        assert_eq!(bytes, original_content);
        pool.close().await;
    }

    #[tokio::test]
    async fn doctor_detects_blob_upload_hash_and_unrecoverable_state_failures() {
        let (root, config, object, content, part, part_content) = setup().await;
        doctor(&config).unwrap();

        fs::write(root.data().join(&object), b"corrupt-blob").unwrap();
        assert!(doctor(&config).is_err());
        fs::write(root.data().join(&object), content).unwrap();

        fs::write(root.data().join(&part), b"corrupt-part").unwrap();
        assert!(doctor(&config).is_err());
        fs::write(root.data().join(&part), part_content).unwrap();

        let connection = Connection::open(root.database()).unwrap();
        connection
            .execute("UPDATE uploads SET commit_state='unknown'", [])
            .unwrap();
        drop(connection);
        assert!(doctor(&config).is_err());
    }
}
