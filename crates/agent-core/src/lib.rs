use std::{
    fs,
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use photo_backup_crypto::prepare_file;
use photo_backup_protocol::{CreateUploadRequest, MediaKind};
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
    #[error("crypto error: {0}")]
    Crypto(#[from] photo_backup_crypto::CryptoError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("job not found")]
    NotFound,
    #[error("invalid media kind: {0}")]
    InvalidMediaKind(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub master_key_b64: String,
    pub dedupe_key_b64: String,
    #[serde(default = "default_part_size")]
    pub part_size: usize,
}

fn default_part_size() -> usize {
    16 * 1024 * 1024
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueueResource {
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
    #[serde(default)]
    pub remove_source_after_prepare: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalPart {
    pub index: u32,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedJob {
    pub job_id: String,
    pub request: CreateUploadRequest,
    pub local_parts: Vec<LocalPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentStats {
    pub discovered: u64,
    pub ready: u64,
    pub uploading: u64,
    pub complete: u64,
    pub retry_wait: u64,
    pub failed: u64,
}

pub struct Agent {
    connection: Mutex<Connection>,
    config: AgentConfig,
}

impl Agent {
    pub fn open(path: impl AsRef<Path>, config: AgentConfig) -> Result<Self, AgentError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                source_asset_id TEXT NOT NULL,
                source_resource_id TEXT NOT NULL,
                media_kind TEXT NOT NULL,
                role TEXT NOT NULL,
                file_path TEXT NOT NULL,
                filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                source_created_at_ms INTEGER NOT NULL,
                modified_ms INTEGER NOT NULL,
                source_size INTEGER NOT NULL,
                metadata_json TEXT,
                remove_source_after_prepare INTEGER NOT NULL DEFAULT 0,
                state TEXT NOT NULL,
                prepared_json TEXT,
                upload_id TEXT,
                retry_count INTEGER NOT NULL DEFAULT 0,
                next_retry_ms INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                updated_at_ms INTEGER NOT NULL,
                UNIQUE(source_asset_id, source_resource_id)
            );
            CREATE TABLE IF NOT EXISTS job_parts (
                job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                part_index INTEGER NOT NULL,
                uploaded INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(job_id, part_index)
            );
            "#,
        )?;
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
        let now = now_ms();
        let connection = self
            .connection
            .lock()
            .expect("agent database mutex poisoned");
        let existing: Option<(String, i64, u64)> = connection
            .query_row(
                "SELECT id, modified_ms, source_size FROM jobs WHERE source_asset_id = ?1 AND source_resource_id = ?2",
                params![input.source_asset_id, input.source_resource_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let id = existing
            .as_ref()
            .map(|value| value.0.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let changed = existing
            .map(|(_, modified, size)| modified != input.modified_ms || size != input.source_size)
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
                return Ok(Some(serde_json::from_str(&json)?));
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
        let output_dir = staging_root.join("encrypted").join(job_id);
        let crypto = prepare_file(
            Path::new(&input.file_path),
            &output_dir,
            &self.config.master_key_b64,
            &self.config.dedupe_key_b64,
            self.config.part_size,
            input.metadata_json.as_deref(),
        )?;
        let request = CreateUploadRequest {
            source_asset_id: input.source_asset_id.clone(),
            source_resource_id: input.source_resource_id.clone(),
            media_kind,
            role: input.role.clone(),
            filename: input.filename.clone(),
            mime_type: input.mime_type.clone(),
            source_created_at_ms: input.source_created_at_ms,
            plaintext_size: crypto.plaintext_size,
            dedup_token: crypto.dedup_token,
            wrapped_key: crypto.wrapped_key,
            key_nonce: crypto.key_nonce,
            nonce_prefix: crypto.nonce_prefix,
            metadata_nonce: crypto.metadata_nonce,
            metadata_ciphertext: crypto.metadata_ciphertext,
            parts: crypto.parts.iter().map(|part| part.spec.clone()).collect(),
        };
        let prepared = PreparedJob {
            job_id: job_id.to_owned(),
            request,
            local_parts: crypto
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
        self.update_job(
            "UPDATE jobs SET state = 'complete', error = NULL, updated_at_ms = ?2 WHERE id = ?1",
            params![job_id, now_ms()],
        )?;
        if let Some(json) = prepared {
            if let Ok(job) = serde_json::from_str::<PreparedJob>(&json) {
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
