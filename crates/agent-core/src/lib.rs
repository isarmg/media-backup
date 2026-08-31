use std::{
    fs,
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

mod database;

use media_backup_crypto::prepare_file;
use media_backup_protocol::{CreateUploadRequest, MediaKind, StorageEncoding};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("content preparation error: {0}")]
    Content(#[from] media_backup_crypto::ContentError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent database is not exactly current: {0}")]
    CurrentState(#[from] anyhow::Error),
    #[error("job not found")]
    NotFound,
    #[error("invalid media kind: {0}")]
    InvalidMediaKind(String),
    #[error("invalid mobile v0.2 contract: {0}")]
    InvalidContract(String),
}

pub const MOBILE_PRODUCT: &str = "media-backup";
pub const MOBILE_APPLICATION_VERSION: &str = "0.2.0";
pub const MOBILE_REVISION: u32 = 1;
pub const MOBILE_STATE_EPOCH: &str = "media-backup-mobile-v0.2-r1";
pub const MOBILE_DATABASE_FILENAME: &str = "agent-v0.2-r1.sqlite";
pub const MOBILE_STAGING_DIRECTORY: &str = "backup-staging-v0.2-r1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    pub product: String,
    pub application_version: String,
    pub revision: u32,
    pub state_epoch: String,
    pub part_size: usize,
}

impl AgentConfig {
    fn validate(&self) -> Result<(), AgentError> {
        validate_contract(
            &self.product,
            &self.application_version,
            self.revision,
            &self.state_epoch,
        )?;
        if self.part_size == 0 {
            return Err(AgentError::InvalidContract(
                "part_size must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnqueueResource {
    pub product: String,
    pub application_version: String,
    pub revision: u32,
    pub state_epoch: String,
    pub source_asset_id: String,
    pub source_resource_id: String,
    pub media_kind: String,
    pub role: String,
    pub file_path: String,
    pub filename: String,
    pub mime_type: String,
    pub source_created_at_ms: i64,
    pub modified_ms: i64,
    pub source_size: u64,
    pub metadata_json: Option<String>,
    pub remove_source_after_prepare: bool,
}

impl EnqueueResource {
    fn validate(&self) -> Result<(), AgentError> {
        validate_contract(
            &self.product,
            &self.application_version,
            self.revision,
            &self.state_epoch,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPart {
    pub index: u32,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedJob {
    pub product: String,
    pub application_version: String,
    pub revision: u32,
    pub state_epoch: String,
    pub job_id: String,
    pub request: CreateUploadRequest,
    pub local_parts: Vec<LocalPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AgentStats {
    pub discovered: u64,
    pub ready: u64,
    pub uploading: u64,
    pub complete: u64,
    pub retry_wait: u64,
    pub failed: u64,
}

fn validate_contract(
    product: &str,
    application_version: &str,
    revision: u32,
    state_epoch: &str,
) -> Result<(), AgentError> {
    if product != MOBILE_PRODUCT {
        return Err(AgentError::InvalidContract(format!(
            "product must be {MOBILE_PRODUCT}"
        )));
    }
    if application_version != MOBILE_APPLICATION_VERSION {
        return Err(AgentError::InvalidContract(format!(
            "application_version must be {MOBILE_APPLICATION_VERSION}"
        )));
    }
    if revision != MOBILE_REVISION {
        return Err(AgentError::InvalidContract(format!(
            "revision must be {MOBILE_REVISION}"
        )));
    }
    if state_epoch != MOBILE_STATE_EPOCH {
        return Err(AgentError::InvalidContract(format!(
            "state_epoch must be {MOBILE_STATE_EPOCH}"
        )));
    }
    Ok(())
}

impl PreparedJob {
    fn validate(&self) -> Result<(), AgentError> {
        validate_contract(
            &self.product,
            &self.application_version,
            self.revision,
            &self.state_epoch,
        )
    }
}

pub struct Agent {
    connection: Mutex<Connection>,
    config: AgentConfig,
}

impl Agent {
    pub fn open(path: impl AsRef<Path>, config: AgentConfig) -> Result<Self, AgentError> {
        // Contract validation deliberately precedes every filesystem or SQLite
        // operation, so a v0.1 payload is a zero-write rejection.
        config.validate()?;
        let connection = database::open_current(path.as_ref())?;
        validate_persisted_jobs(&connection)?;
        connection.execute(
            "UPDATE jobs SET state = 'ready', upload_id = NULL WHERE state IN ('preparing', 'uploading') AND prepared_json IS NOT NULL",
            [],
        )?;
        connection.execute(
            "UPDATE jobs SET state = 'discovered' WHERE state = 'preparing' AND prepared_json IS NULL",
            [],
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
            config,
        })
    }

    pub fn needs_resource(
        &self,
        source_asset_id: &str,
        source_resource_id: &str,
        modified_ms: i64,
    ) -> Result<bool, AgentError> {
        let connection = self
            .connection
            .lock()
            .expect("agent database mutex poisoned");
        let existing: Option<(String, i64)> = connection
            .query_row(
                "SELECT state, modified_ms FROM jobs WHERE source_asset_id = ?1 AND source_resource_id = ?2",
                params![source_asset_id, source_resource_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(
            !matches!(existing, Some((state, known_modified)) if state == "complete" && known_modified == modified_ms),
        )
    }

    pub fn enqueue(&self, input: EnqueueResource) -> Result<String, AgentError> {
        input.validate()?;
        let now = now_ms();
        let connection = self
            .connection
            .lock()
            .expect("agent database mutex poisoned");
        let existing: Option<(String, i64, u64, String)> = connection
            .query_row(
                "SELECT id, modified_ms, source_size, state FROM jobs WHERE source_asset_id = ?1 AND source_resource_id = ?2",
                params![input.source_asset_id, input.source_resource_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let id = existing
            .as_ref()
            .map(|value| value.0.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let changed = existing
            .map(|(_, modified, size, state)| {
                modified != input.modified_ms || size != input.source_size || state != "complete"
            })
            .unwrap_or(true);
        if changed {
            connection.execute(
                r#"
                INSERT INTO jobs (
                    id, source_asset_id, source_resource_id, media_kind, role, file_path,
                    filename, mime_type, source_created_at_ms, modified_ms, source_size,
                    metadata_json, remove_source_after_prepare, state, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'discovered', ?14)
                ON CONFLICT(source_asset_id, source_resource_id) DO UPDATE SET
                    media_kind = excluded.media_kind,
                    role = excluded.role,
                    file_path = excluded.file_path,
                    filename = excluded.filename,
                    mime_type = excluded.mime_type,
                    source_created_at_ms = excluded.source_created_at_ms,
                    modified_ms = excluded.modified_ms,
                    source_size = excluded.source_size,
                    metadata_json = excluded.metadata_json,
                    remove_source_after_prepare = excluded.remove_source_after_prepare,
                    state = 'discovered',
                    prepared_json = NULL,
                    upload_id = NULL,
                    retry_count = 0,
                    next_retry_ms = 0,
                    error = NULL,
                    updated_at_ms = excluded.updated_at_ms
                "#,
                params![
                    id,
                    input.source_asset_id,
                    input.source_resource_id,
                    input.media_kind,
                    input.role,
                    input.file_path,
                    input.filename,
                    input.mime_type,
                    input.source_created_at_ms,
                    input.modified_ms,
                    input.source_size,
                    input.metadata_json,
                    input.remove_source_after_prepare as i32,
                    now,
                ],
            )?;
        }
        Ok(id)
    }

    pub fn next_prepared(
        &self,
        staging_root: impl AsRef<Path>,
    ) -> Result<Option<PreparedJob>, AgentError> {
        {
            let connection = self
                .connection
                .lock()
                .expect("agent database mutex poisoned");
            let existing: Option<String> = connection
                .query_row(
                    "SELECT prepared_json FROM jobs WHERE state = 'ready' ORDER BY updated_at_ms LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(json) = existing {
                let prepared: PreparedJob = serde_json::from_str(&json)?;
                prepared.validate()?;
                return Ok(Some(prepared));
            }
        }

        let now = now_ms();
        let row = {
            let connection = self
                .connection
                .lock()
                .expect("agent database mutex poisoned");
            let row: Option<(String, EnqueueResource)> = connection
                .query_row(
                    r#"
                    SELECT id, source_asset_id, source_resource_id, media_kind, role, file_path,
                           filename, mime_type, source_created_at_ms, modified_ms, source_size,
                           metadata_json, remove_source_after_prepare
                    FROM jobs
                    WHERE state = 'discovered' OR (state = 'retry_wait' AND next_retry_ms <= ?1)
                    ORDER BY updated_at_ms
                    LIMIT 1
                    "#,
                    params![now],
                    |row| {
                        Ok((
                            row.get(0)?,
                            EnqueueResource {
                                product: MOBILE_PRODUCT.to_owned(),
                                application_version: MOBILE_APPLICATION_VERSION.to_owned(),
                                revision: MOBILE_REVISION,
                                state_epoch: MOBILE_STATE_EPOCH.to_owned(),
                                source_asset_id: row.get(1)?,
                                source_resource_id: row.get(2)?,
                                media_kind: row.get(3)?,
                                role: row.get(4)?,
                                file_path: row.get(5)?,
                                filename: row.get(6)?,
                                mime_type: row.get(7)?,
                                source_created_at_ms: row.get(8)?,
                                modified_ms: row.get(9)?,
                                source_size: row.get(10)?,
                                metadata_json: row.get(11)?,
                                remove_source_after_prepare: row.get::<_, i32>(12)? != 0,
                            },
                        ))
                    },
                )
                .optional()?;
            if let Some((id, _)) = &row {
                connection.execute(
                    "UPDATE jobs SET state = 'preparing', updated_at_ms = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
            }
            row
        };

        let Some((job_id, input)) = row else {
            return Ok(None);
        };
        match self.prepare_job(&job_id, &input, staging_root.as_ref()) {
            Ok(prepared) => Ok(Some(prepared)),
            Err(error) => {
                self.mark_failed(&job_id, &error.to_string(), true)?;
                Err(error)
            }
        }
    }

    fn prepare_job(
        &self,
        job_id: &str,
        input: &EnqueueResource,
        staging_root: &Path,
    ) -> Result<PreparedJob, AgentError> {
        let media_kind = match input.media_kind.as_str() {
            "photo" => MediaKind::Photo,
            "video" => MediaKind::Video,
            "other" => MediaKind::Other,
            value => return Err(AgentError::InvalidMediaKind(value.to_owned())),
        };
        let output_dir = staging_root.join("prepared").join(job_id);
        let content = prepare_file(
            Path::new(&input.file_path),
            &output_dir,
            self.config.part_size,
        )?;
        let request = CreateUploadRequest {
            source_asset_id: input.source_asset_id.clone(),
            source_resource_id: input.source_resource_id.clone(),
            media_kind,
            role: input.role.clone(),
            filename: input.filename.clone(),
            mime_type: input.mime_type.clone(),
            source_created_at_ms: input.source_created_at_ms,
            storage_encoding: StorageEncoding::PlainV1,
            content_size: content.content_size,
            content_blake3: content.content_blake3,
            metadata: input
                .metadata_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            parts: content.parts.iter().map(|part| part.spec.clone()).collect(),
        };
        let prepared = PreparedJob {
            product: MOBILE_PRODUCT.to_owned(),
            application_version: MOBILE_APPLICATION_VERSION.to_owned(),
            revision: MOBILE_REVISION,
            state_epoch: MOBILE_STATE_EPOCH.to_owned(),
            job_id: job_id.to_owned(),
            request,
            local_parts: content
                .parts
                .iter()
                .map(|part| LocalPart {
                    index: part.spec.index,
                    path: part.path.to_string_lossy().into_owned(),
                })
                .collect(),
        };
        let prepared_json = serde_json::to_string(&prepared)?;
        let connection = self
            .connection
            .lock()
            .expect("agent database mutex poisoned");
        connection.execute(
            "UPDATE jobs SET state = 'ready', prepared_json = ?2, error = NULL, updated_at_ms = ?3 WHERE id = ?1",
            params![job_id, prepared_json, now_ms()],
        )?;
        if input.remove_source_after_prepare {
            let _ = fs::remove_file(&input.file_path);
        }
        Ok(prepared)
    }

    pub fn mark_upload(&self, job_id: &str, upload_id: &str) -> Result<(), AgentError> {
        self.update_job(
            "UPDATE jobs SET state = 'uploading', upload_id = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![job_id, upload_id, now_ms()],
        )
    }

    pub fn mark_part_uploaded(&self, job_id: &str, index: u32) -> Result<(), AgentError> {
        let connection = self
            .connection
            .lock()
            .expect("agent database mutex poisoned");
        connection.execute(
            "INSERT INTO job_parts(job_id, part_index, uploaded) VALUES (?1, ?2, 1) ON CONFLICT(job_id, part_index) DO UPDATE SET uploaded = 1",
            params![job_id, index],
        )?;
        Ok(())
    }

    pub fn mark_complete(&self, job_id: &str) -> Result<(), AgentError> {
        let prepared: Option<String> = {
            let connection = self
                .connection
                .lock()
                .expect("agent database mutex poisoned");
            connection
                .query_row(
                    "SELECT prepared_json FROM jobs WHERE id = ?1",
                    params![job_id],
                    |row| row.get(0),
                )
                .optional()?
        };
        let prepared = prepared
            .as_deref()
            .map(serde_json::from_str::<PreparedJob>)
            .transpose()?;
        if let Some(job) = &prepared {
            job.validate()?;
        }
        self.update_job(
            "UPDATE jobs SET state = 'complete', error = NULL, updated_at_ms = ?2 WHERE id = ?1",
            params![job_id, now_ms()],
        )?;
        if let Some(job) = prepared {
            for part in &job.local_parts {
                let _ = fs::remove_file(&part.path);
            }
            if let Some(parent) = job
                .local_parts
                .first()
                .and_then(|part| Path::new(&part.path).parent())
            {
                let _ = fs::remove_dir(parent);
            }
        }
        Ok(())
    }

    pub fn mark_failed(
        &self,
        job_id: &str,
        error: &str,
        retryable: bool,
    ) -> Result<(), AgentError> {
        let connection = self
            .connection
            .lock()
            .expect("agent database mutex poisoned");
        let retries: u32 = connection
            .query_row(
                "SELECT retry_count FROM jobs WHERE id = ?1",
                params![job_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(AgentError::NotFound)?;
        let state = if retryable { "retry_wait" } else { "failed" };
        let exponent = retries.min(10);
        let delay = (2_000_i64 * (1_i64 << exponent)).min(3_600_000);
        connection.execute(
            "UPDATE jobs SET state = ?2, retry_count = retry_count + 1, next_retry_ms = ?3, error = ?4, upload_id = NULL, updated_at_ms = ?5 WHERE id = ?1",
            params![job_id, state, now_ms() + delay, error, now_ms()],
        )?;
        Ok(())
    }

    pub fn stats(&self) -> Result<AgentStats, AgentError> {
        let connection = self
            .connection
            .lock()
            .expect("agent database mutex poisoned");
        let mut statement =
            connection.prepare("SELECT state, COUNT(*) FROM jobs GROUP BY state")?;
        let mut rows = statement.query([])?;
        let mut stats = AgentStats::default();
        while let Some(row) = rows.next()? {
            let state: String = row.get(0)?;
            let count: u64 = row.get(1)?;
            match state.as_str() {
                "discovered" | "preparing" => stats.discovered += count,
                "ready" => stats.ready += count,
                "uploading" => stats.uploading += count,
                "complete" => stats.complete += count,
                "retry_wait" => stats.retry_wait += count,
                "failed" => stats.failed += count,
                _ => {}
            }
        }
        Ok(stats)
    }

    fn update_job(&self, sql: &str, values: impl rusqlite::Params) -> Result<(), AgentError> {
        let connection = self
            .connection
            .lock()
            .expect("agent database mutex poisoned");
        if connection.execute(sql, values)? == 0 {
            return Err(AgentError::NotFound);
        }
        Ok(())
    }
}

fn validate_persisted_jobs(connection: &Connection) -> Result<(), AgentError> {
    let mut statement = connection
        .prepare("SELECT prepared_json FROM jobs WHERE prepared_json IS NOT NULL ORDER BY id")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        let prepared: PreparedJob = serde_json::from_str(&row?)?;
        prepared.validate()?;
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs::File};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn current_config(part_size: usize) -> AgentConfig {
        AgentConfig {
            product: MOBILE_PRODUCT.to_owned(),
            application_version: MOBILE_APPLICATION_VERSION.to_owned(),
            revision: MOBILE_REVISION,
            state_epoch: MOBILE_STATE_EPOCH.to_owned(),
            part_size,
        }
    }

    fn current_resource() -> EnqueueResource {
        EnqueueResource {
            product: MOBILE_PRODUCT.to_owned(),
            application_version: MOBILE_APPLICATION_VERSION.to_owned(),
            revision: MOBILE_REVISION,
            state_epoch: MOBILE_STATE_EPOCH.to_owned(),
            source_asset_id: "asset".to_owned(),
            source_resource_id: "resource".to_owned(),
            media_kind: "photo".to_owned(),
            role: "primary".to_owned(),
            file_path: String::new(),
            filename: "source.jpg".to_owned(),
            mime_type: "image/jpeg".to_owned(),
            source_created_at_ms: 1,
            modified_ms: 1,
            source_size: 0,
            metadata_json: None,
            remove_source_after_prepare: false,
        }
    }

    #[test]
    fn prepared_job_contains_original_plaintext_parts() {
        let root = std::env::temp_dir().join(format!("media-backup-agent-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("photo.jpg");
        let original = b"server storage must contain these exact original bytes";
        fs::write(&source, original).unwrap();
        let agent = Agent::open(root.join("agent.sqlite"), current_config(11)).unwrap();
        agent
            .enqueue(EnqueueResource {
                product: MOBILE_PRODUCT.to_owned(),
                application_version: MOBILE_APPLICATION_VERSION.to_owned(),
                revision: MOBILE_REVISION,
                state_epoch: MOBILE_STATE_EPOCH.to_owned(),
                source_asset_id: "asset-1".to_owned(),
                source_resource_id: "resource-1".to_owned(),
                media_kind: "photo".to_owned(),
                role: "primary".to_owned(),
                file_path: source.to_string_lossy().into_owned(),
                filename: "photo.jpg".to_owned(),
                mime_type: "image/jpeg".to_owned(),
                source_created_at_ms: 123,
                modified_ms: 456,
                source_size: original.len() as u64,
                metadata_json: Some(r#"{"favorite":true}"#.to_owned()),
                remove_source_after_prepare: true,
            })
            .unwrap();
        let prepared = agent.next_prepared(&root).unwrap().unwrap();
        assert_eq!(prepared.request.storage_encoding, StorageEncoding::PlainV1);
        assert_eq!(prepared.request.content_size, original.len() as u64);
        assert_eq!(
            prepared.request.content_blake3,
            blake3::hash(original).to_hex().to_string()
        );
        let assembled = prepared
            .local_parts
            .iter()
            .flat_map(|part| fs::read(&part.path).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(assembled, original);
        let mut persisted = serde_json::to_value(&prepared).unwrap();
        persisted["legacy_cipher_metadata"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<PreparedJob>(persisted).is_err());
        let mut persisted = serde_json::to_value(&prepared).unwrap();
        persisted["application_version"] = serde_json::Value::String("0.1.0".to_owned());
        assert!(serde_json::from_value::<PreparedJob>(persisted)
            .unwrap()
            .validate()
            .is_err());
        assert!(!source.exists());
        drop(agent);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mobile_json_contract_has_no_defaults_or_unknown_field_tolerance() {
        assert_eq!(env!("CARGO_PKG_VERSION"), MOBILE_APPLICATION_VERSION);
        let mut config = serde_json::to_value(current_config(16)).unwrap();
        config.as_object_mut().unwrap().remove("part_size");
        assert!(serde_json::from_value::<AgentConfig>(config).is_err());

        let mut config = serde_json::to_value(current_config(16)).unwrap();
        config["master_key_b64"] = serde_json::Value::String("legacy".to_owned());
        assert!(serde_json::from_value::<AgentConfig>(config).is_err());

        let mut resource = serde_json::to_value(current_resource()).unwrap();
        resource
            .as_object_mut()
            .unwrap()
            .remove("remove_source_after_prepare");
        assert!(serde_json::from_value::<EnqueueResource>(resource).is_err());
    }

    #[test]
    fn fresh_agent_database_has_exact_current_metadata_and_schema() {
        let root = tempfile::tempdir().unwrap();
        let database_path = root.path().join("agent.sqlite3");
        drop(Agent::open(&database_path, current_config(16)).unwrap());

        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&database_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let connection = Connection::open(&database_path).unwrap();
        let metadata: (String, String, i64, String) = connection
            .query_row(
                "SELECT application, application_version, schema_revision, schema_sha256
                 FROM product_metadata WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(metadata.0, database::tests_support::APPLICATION);
        assert_eq!(metadata.1, env!("CARGO_PKG_VERSION"));
        assert_eq!(metadata.2, database::CURRENT_SCHEMA_REVISION);
        assert_eq!(metadata.3, database::CURRENT_SCHEMA_SHA256);
        assert_eq!(
            database::tests_support::fingerprint(&connection).unwrap(),
            database::CURRENT_SCHEMA_SHA256
        );
    }

    #[test]
    fn old_or_empty_agent_databases_are_rejected_without_byte_changes() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old.sqlite3");
        let connection = Connection::open(&old).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE jobs(
                    id TEXT PRIMARY KEY,
                    prepared_json TEXT,
                    state TEXT NOT NULL
                 );",
            )
            .unwrap();
        drop(connection);
        secure_permissions(&old);
        assert_rejected_without_byte_changes(&old);

        let empty = root.path().join("empty.sqlite3");
        File::create(&empty).unwrap();
        secure_permissions(&empty);
        assert_rejected_without_byte_changes(&empty);
        assert_eq!(fs::metadata(empty).unwrap().len(), 0);
    }

    #[test]
    fn nonexact_agent_metadata_and_schema_are_read_only_rejections() {
        for (name, statement) in [
            (
                "wrong-application",
                "UPDATE product_metadata SET application = 'media-backup'",
            ),
            (
                "old-version",
                "UPDATE product_metadata SET application_version = '0.1.0'",
            ),
            (
                "wrong-revision",
                "UPDATE product_metadata SET schema_revision = 2",
            ),
            (
                "wrong-fingerprint",
                "UPDATE product_metadata SET schema_sha256 = 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'",
            ),
            (
                "schema-drift",
                "CREATE TABLE unexpected_agent_table(id INTEGER)",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join(format!("{name}.sqlite3"));
            database::tests_support::initialize(&path).unwrap();
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch("PRAGMA journal_mode=DELETE;").unwrap();
            connection.execute_batch(statement).unwrap();
            drop(connection);
            assert_rejected_without_byte_changes(&path);
        }
    }

    #[test]
    fn agent_metadata_table_contract_is_exact_and_rejection_is_read_only() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("metadata-contract.sqlite3");
        database::tests_support::initialize(&path).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA journal_mode=DELETE;")
            .unwrap();
        connection
            .execute_batch(&format!(
                "DROP TABLE product_metadata;
                 CREATE TABLE product_metadata (
                     singleton INTEGER PRIMARY KEY,
                     application TEXT NOT NULL,
                     application_version TEXT NOT NULL,
                     schema_revision INTEGER NOT NULL,
                     schema_sha256 TEXT NOT NULL
                 );
                 INSERT INTO product_metadata VALUES (
                     1, '{}', '{}', {}, '{}'
                 );",
                database::tests_support::APPLICATION,
                env!("CARGO_PKG_VERSION"),
                database::CURRENT_SCHEMA_REVISION,
                database::CURRENT_SCHEMA_SHA256,
            ))
            .unwrap();
        drop(connection);
        assert_rejected_without_byte_changes(&path);
    }

    #[test]
    fn absent_main_with_sidecar_and_file_aliases_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing.sqlite3");
        let orphan = sidecar(&missing, "-wal");
        fs::write(&orphan, b"orphan-generation-evidence").unwrap();
        let before = fs::read(&orphan).unwrap();
        let result = Agent::open(&missing, current_config(16));
        assert!(result.is_err());
        assert!(!missing.exists());
        assert!(fs::read(orphan).unwrap() == before);

        let current = root.path().join("current.sqlite3");
        database::tests_support::initialize(&current).unwrap();
        let before = generation_bytes(&current);
        let symbolic = root.path().join("symbolic.sqlite3");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&current, &symbolic).unwrap();
            assert!(Agent::open(&symbolic, current_config(16)).is_err());
            assert!(generation_bytes(&current) == before);

            let hard = root.path().join("hard.sqlite3");
            fs::hard_link(&current, &hard).unwrap();
            assert!(Agent::open(&hard, current_config(16)).is_err());
            assert!(generation_bytes(&current) == before);
        }
    }

    #[test]
    fn noncurrent_schema_in_wal_is_rejected_without_byte_changes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("drift-wal.sqlite3");
        database::tests_support::initialize(&path).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE unexpected_wal_table(id INTEGER);",
            )
            .unwrap();
        assert!(sidecar(&path, "-wal").exists());
        assert_rejected_without_byte_changes(&path);
        drop(connection);
    }

    #[test]
    fn current_schema_committed_in_wal_is_accepted() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("current-wal.sqlite3");
        database::tests_support::initialize(&path).unwrap();
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA wal_autocheckpoint=0;
                 INSERT INTO jobs(
                     id, source_asset_id, source_resource_id, media_kind, role, file_path,
                     filename, mime_type, source_created_at_ms, modified_ms, source_size,
                     state, updated_at_ms
                 ) VALUES (
                     'wal-job', 'wal-asset', 'wal-resource', 'photo', 'primary', '/tmp/source',
                     'source.jpg', 'image/jpeg', 1, 1, 1, 'complete', 1
                 );",
            )
            .unwrap();
        assert!(sidecar(&path, "-wal").exists());
        let agent = Agent::open(&path, current_config(16)).unwrap();
        assert!(!agent
            .needs_resource("wal-asset", "wal-resource", 1)
            .unwrap());
        drop(agent);
        drop(writer);
    }

    #[test]
    fn current_crash_recovery_does_not_use_persisted_json_heuristics() {
        let root = tempfile::tempdir().unwrap();
        let database_path = root.path().join("agent.sqlite3");
        let source = root.path().join("source.jpg");
        fs::write(&source, b"current-content").unwrap();
        let agent = Agent::open(&database_path, current_config(16)).unwrap();
        let job_id = agent
            .enqueue(EnqueueResource {
                product: MOBILE_PRODUCT.to_owned(),
                application_version: MOBILE_APPLICATION_VERSION.to_owned(),
                revision: MOBILE_REVISION,
                state_epoch: MOBILE_STATE_EPOCH.to_owned(),
                source_asset_id: "asset".to_owned(),
                source_resource_id: "resource".to_owned(),
                media_kind: "photo".to_owned(),
                role: "primary".to_owned(),
                file_path: source.to_string_lossy().into_owned(),
                filename: "source.jpg".to_owned(),
                mime_type: "image/jpeg".to_owned(),
                source_created_at_ms: 1,
                modified_ms: 1,
                source_size: 15,
                metadata_json: None,
                remove_source_after_prepare: false,
            })
            .unwrap();
        agent.next_prepared(root.path()).unwrap().unwrap();
        agent
            .mark_upload(&job_id, &Uuid::new_v4().to_string())
            .unwrap();
        drop(agent);

        let agent = Agent::open(&database_path, current_config(16)).unwrap();
        let state: String = agent
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT state FROM jobs WHERE id = ?1", [&job_id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(state, "ready");
    }

    #[test]
    fn noncurrent_persisted_job_is_rejected_before_crash_recovery_updates() {
        let root = tempfile::tempdir().unwrap();
        let database_path = root.path().join("agent.sqlite3");
        let source = root.path().join("source.jpg");
        fs::write(&source, b"current-content").unwrap();
        let agent = Agent::open(&database_path, current_config(16)).unwrap();
        let mut input = current_resource();
        input.file_path = source.to_string_lossy().into_owned();
        input.source_size = 15;
        let job_id = agent.enqueue(input).unwrap();
        agent.next_prepared(root.path()).unwrap().unwrap();
        drop(agent);

        let connection = Connection::open(&database_path).unwrap();
        let persisted: String = connection
            .query_row(
                "SELECT prepared_json FROM jobs WHERE id = ?1",
                [&job_id],
                |row| row.get(0),
            )
            .unwrap();
        let mut persisted: serde_json::Value = serde_json::from_str(&persisted).unwrap();
        persisted["legacy_cipher_metadata"] = serde_json::Value::Bool(true);
        connection
            .execute(
                "UPDATE jobs SET state = 'uploading', prepared_json = ?2 WHERE id = ?1",
                params![job_id, persisted.to_string()],
            )
            .unwrap();
        drop(connection);

        assert!(Agent::open(&database_path, current_config(16)).is_err());
        let connection = Connection::open(&database_path).unwrap();
        let state: String = connection
            .query_row("SELECT state FROM jobs WHERE id = ?1", [&job_id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(state, "uploading");
    }

    fn assert_rejected_without_byte_changes(path: &Path) {
        let before = generation_bytes(path);
        let result = Agent::open(path, current_config(16));
        let error = match result {
            Ok(_) => panic!("non-current agent database was accepted"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("current")
                || error.to_string().contains("product_metadata")
                || error.to_string().contains("schema")
                || error.to_string().contains("database"),
            "rejection did not identify the current-state boundary: {error}"
        );
        assert!(
            generation_bytes(path) == before,
            "rejecting a non-current agent database changed its SQLite generation"
        );
    }

    fn generation_bytes(path: &Path) -> BTreeMap<String, Vec<u8>> {
        database::tests_support::generation_paths(path)
            .into_iter()
            .filter(|candidate| candidate.exists())
            .map(|candidate| {
                (
                    candidate
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(candidate).unwrap(),
                )
            })
            .collect()
    }

    fn sidecar(path: &Path, suffix: &str) -> std::path::PathBuf {
        let mut value = path.as_os_str().to_os_string();
        value.push(suffix);
        value.into()
    }

    fn secure_permissions(path: &Path) {
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
