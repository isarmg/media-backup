use std::{
    future::poll_fn,
    path::{Component, Path, PathBuf},
    pin::Pin,
};

use axum::{
    body::{Body, HttpBody},
    http::StatusCode,
};
use photo_backup_protocol::UploadPartSpec;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};
use uuid::Uuid;

use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct LocalStorage {
    root: PathBuf,
}

impl LocalStorage {
    pub async fn new(root: PathBuf) -> Result<Self, AppError> {
        fs::create_dir_all(root.join("uploads")).await?;
        fs::create_dir_all(root.join("blobs")).await?;
        Ok(Self { root })
    }

    fn upload_dir(&self, upload_id: Uuid) -> PathBuf {
        self.root.join("uploads").join(upload_id.to_string())
    }

    fn part_path(&self, upload_id: Uuid, index: u32) -> PathBuf {
        self.upload_dir(upload_id).join(format!("{index:08}.part"))
    }

    pub async fn put_part(
        &self,
        upload_id: Uuid,
        spec: &UploadPartSpec,
        mut body: Body,
        max_part_bytes: usize,
    ) -> Result<(), AppError> {
        let directory = self.upload_dir(upload_id);
        fs::create_dir_all(&directory).await?;
        let final_path = self.part_path(upload_id, spec.index);
        let temporary_path = directory.join(format!(
            ".{index:08}.{request_id}.tmp",
            index = spec.index,
            request_id = Uuid::new_v4()
        ));
        let (mut temporary, mut file) = TemporaryPart::create(temporary_path)?;
        let max_part_bytes = u64::try_from(max_part_bytes).unwrap_or(u64::MAX);
        let write_result = async {
            let mut total = 0_u64;
            let mut hasher = blake3::Hasher::new();
            while let Some(frame) = poll_fn(|context| Pin::new(&mut body).poll_frame(context)).await
            {
                let frame = frame.map_err(|error| {
                    tracing::warn!(?error, "failed to read upload part body");
                    AppError::bad_request("failed to read part body")
                })?;
                let Ok(chunk) = frame.into_data() else {
                    continue;
                };
                let chunk_size = u64::try_from(chunk.len())
                    .map_err(|_| AppError::bad_request("part is too large"))?;
                total = total
                    .checked_add(chunk_size)
                    .ok_or_else(|| AppError::bad_request("part is too large"))?;
                if total > max_part_bytes {
                    return Err(AppError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "part exceeds server limit",
                    ));
                }
                if total > spec.size {
                    return Err(AppError::bad_request("part size does not match manifest"));
                }
                file.write_all(&chunk).await?;
                hasher.update(&chunk);
            }
            if total != spec.size {
                return Err(AppError::bad_request("part size does not match manifest"));
            }
            if hasher.finalize().to_hex().as_str() != spec.blake3 {
                return Err(AppError::bad_request("part hash does not match manifest"));
            }
            file.sync_all().await?;
            Ok::<(), AppError>(())
        }
        .await;
        drop(file);

        if let Err(error) = write_result {
            cleanup_temporary_best_effort(&mut temporary, &directory).await;
            return Err(error);
        }

        match fs::hard_link(temporary.path(), &final_path).await {
            Ok(()) => {
                cleanup_temporary(&mut temporary, &directory).await?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = existing_part_matches(&final_path, spec).await;
                cleanup_temporary(&mut temporary, &directory).await?;
                match existing? {
                    Some(true) => Ok(()),
                    Some(false) | None => Err(part_conflict()),
                }
            }
            Err(error) => {
                cleanup_temporary_best_effort(&mut temporary, &directory).await;
                Err(error.into())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finalize(
        &self,
        account_id: Uuid,
        configured_path: &str,
        upload_id: Uuid,
        parts: &[UploadPartSpec],
        filename: &str,
        expected_size: u64,
        expected_blake3: &str,
    ) -> Result<(String, u64), AppError> {
        let account_dir = if configured_path.trim().is_empty() {
            self.root.join("blobs").join(account_id.to_string())
        } else {
            self.resolve_account_root(configured_path)?
        };
        fs::create_dir_all(&account_dir).await?;
        let extension = Path::new(filename)
            .extension()
            .and_then(|value| value.to_str())
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 16
                    && value
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric())
            })
            .unwrap_or("media");
        let final_path = account_dir.join(format!("{upload_id}.{extension}"));
        if fs::try_exists(&final_path).await? {
            let size = fs::metadata(&final_path).await?.len();
            return Ok((storage_reference(&self.root, &final_path), size));
        }
        let temporary = account_dir.join(format!("{upload_id}.tmp"));
        let mut output = fs::File::create(&temporary).await?;
        let mut total = 0_u64;
        for part in parts {
            let part_path = self.part_path(upload_id, part.index);
            let mut input = fs::File::open(&part_path).await.map_err(|_| {
                AppError::conflict(format!("part {} is missing from storage", part.index))
            })?;
            total += tokio::io::copy(&mut input, &mut output).await?;
        }
        output.sync_all().await?;
        drop(output);
        if total != expected_size {
            let _ = fs::remove_file(&temporary).await;
            return Err(AppError::conflict(
                "assembled content size does not match manifest",
            ));
        }
        let actual_hash = hash_file(&temporary).await?;
        if actual_hash != expected_blake3 {
            let _ = fs::remove_file(&temporary).await;
            return Err(AppError::conflict(
                "assembled content hash does not match manifest",
            ));
        }
        fs::rename(&temporary, &final_path).await?;
        Ok((storage_reference(&self.root, &final_path), total))
    }

    pub async fn open_blob(&self, relative_path: &str) -> Result<fs::File, AppError> {
        let path = Path::new(relative_path);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            validate_relative(path)?;
            self.root.join(path)
        };
        Ok(fs::File::open(resolved).await?)
    }

    pub async fn remove_blob(
        &self,
        configured_account_path: &str,
        storage_path: &str,
    ) -> Result<(), AppError> {
        let account_root = self.resolve_account_root(configured_account_path)?;
        let stored = PathBuf::from(storage_path);
        let resolved = if stored.is_absolute() {
            stored
        } else {
            validate_relative(&stored)?;
            self.root.join(stored)
        };
        if !resolved.starts_with(&account_root) {
            return Err(AppError::bad_request(
                "blob path escapes the account storage root",
            ));
        }
        match fs::remove_file(resolved).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn validate_account_path(&self, configured_path: &str) -> Result<(), AppError> {
        let directory = self.resolve_account_root(configured_path)?;
        fs::create_dir_all(&directory).await?;
        let probe = directory.join(format!(".photo-backup-write-test-{}", Uuid::new_v4()));
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&probe)
            .await?;
        file.write_all(b"ok").await?;
        drop(file);
        fs::remove_file(probe).await?;
        Ok(())
    }

    /// Exercise the same local create/write/fsync/remove path required by uploads.
    pub async fn probe_readiness(&self) -> Result<(), AppError> {
        let probe = self
            .root
            .join(format!(".photo-backup-readiness-{}", Uuid::new_v4()));
        let operation = async {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&probe)
                .await?;
            file.write_all(b"photo-backup-readiness-v1").await?;
            file.sync_all().await?;
            drop(file);
            Ok::<(), AppError>(())
        }
        .await;
        let cleanup = fs::remove_file(&probe).await;
        operation?;
        cleanup?;
        fs::File::open(&self.root).await?.sync_all().await?;
        Ok(())
    }

    fn resolve_account_root(&self, configured_path: &str) -> Result<PathBuf, AppError> {
        let value = configured_path.trim();
        if value.is_empty() {
            return Err(AppError::bad_request("storage_path is required"));
        }
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Ok(path);
        }
        validate_relative(&path)?;
        Ok(self.root.join(path))
    }
}

async fn hash_file(path: &Path) -> Result<String, AppError> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

async fn existing_part_matches(
    path: &Path,
    spec: &UploadPartSpec,
) -> Result<Option<bool>, AppError> {
    let mut file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut total = 0_u64;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > spec.size {
            return Ok(Some(false));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(Some(
        total == spec.size && hasher.finalize().to_hex().as_str() == spec.blake3,
    ))
}

fn part_conflict() -> AppError {
    AppError::conflict("stored part conflicts with upload manifest")
}

struct TemporaryPart {
    path: PathBuf,
    remove_on_drop: bool,
}

impl TemporaryPart {
    fn create(path: PathBuf) -> Result<(Self, fs::File), AppError> {
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        Ok((
            Self {
                path,
                remove_on_drop: true,
            },
            fs::File::from_std(file),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    async fn remove(&mut self) -> Result<(), AppError> {
        match fs::remove_file(&self.path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        self.remove_on_drop = false;
        Ok(())
    }
}

impl Drop for TemporaryPart {
    fn drop(&mut self) {
        if self.remove_on_drop {
            match std::fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => tracing::warn!(
                    ?error,
                    path = %self.path.display(),
                    "failed to clean temporary upload part"
                ),
            }
        }
    }
}

async fn cleanup_temporary(
    temporary: &mut TemporaryPart,
    directory: &Path,
) -> Result<(), AppError> {
    let cleanup = temporary.remove().await;
    let sync = sync_directory(directory).await;
    cleanup?;
    sync
}

async fn cleanup_temporary_best_effort(temporary: &mut TemporaryPart, directory: &Path) {
    if let Err(error) = cleanup_temporary(temporary, directory).await {
        tracing::warn!(?error, "failed to durably clean temporary upload part");
    }
}

async fn sync_directory(directory: &Path) -> Result<(), AppError> {
    fs::File::open(directory).await?.sync_all().await?;
    Ok(())
}

fn validate_relative(path: &Path) -> Result<(), AppError> {
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(AppError::bad_request("invalid relative storage path"));
    }
    Ok(())
}

fn storage_reference(root: &Path, path: &Path) -> String {
    if path.starts_with(root) {
        relative_to(root, path)
    } else {
        path.to_string_lossy().to_string()
    }
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::{io::AsyncReadExt, task::JoinSet};
    use tokio_util::io::ReaderStream;

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("photo-backup-parts-{}", Uuid::new_v4()));
            Self { path }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn part_spec(index: u32, content: &[u8]) -> UploadPartSpec {
        UploadPartSpec {
            index,
            size: content.len() as u64,
            blake3: blake3::hash(content).to_hex().to_string(),
        }
    }

    fn chunked_body(byte: u8, size: usize, frame_size: usize) -> Body {
        let reader = tokio::io::repeat(byte).take(size as u64);
        Body::from_stream(ReaderStream::with_capacity(reader, frame_size))
    }

    async fn entry_names(directory: &Path) -> Vec<String> {
        let mut entries = match fs::read_dir(directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => panic!("read test upload directory: {error}"),
        };
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.expect("read test entry") {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        names
    }

    async fn assert_no_temporary_parts(storage: &LocalStorage, upload_id: Uuid) {
        let names = entry_names(&storage.upload_dir(upload_id)).await;
        assert!(
            names.iter().all(|name| !name.ends_with(".tmp")),
            "temporary parts were not cleaned: {names:?}"
        );
    }

    #[tokio::test]
    async fn streams_large_multiframe_part_and_cleans_rejected_temporary_files() {
        let root = TestRoot::new();
        let storage = LocalStorage::new(root.path.clone())
            .await
            .expect("create test storage");

        let upload_id = Uuid::new_v4();
        let size = 256 * 1024 + 17;
        let content = vec![0x5a; size];
        let spec = part_spec(0, &content);
        storage
            .put_part(upload_id, &spec, chunked_body(0x5a, size, 4096), size)
            .await
            .expect("store multi-frame part");
        assert_eq!(
            fs::metadata(storage.part_path(upload_id, spec.index))
                .await
                .expect("read stored part metadata")
                .len(),
            size as u64
        );
        assert_eq!(
            hash_file(&storage.part_path(upload_id, spec.index))
                .await
                .expect("hash stored multi-frame part"),
            spec.blake3
        );
        assert_no_temporary_parts(&storage, upload_id).await;

        let rejected_upload_id = Uuid::new_v4();
        let rejected = storage
            .put_part(
                rejected_upload_id,
                &spec,
                chunked_body(0x5b, size, 4096),
                size,
            )
            .await
            .expect_err("reject part with mismatched hash");
        assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
        assert!(
            !fs::try_exists(storage.part_path(rejected_upload_id, spec.index))
                .await
                .expect("check rejected final part")
        );
        assert_no_temporary_parts(&storage, rejected_upload_id).await;

        let limited_upload_id = Uuid::new_v4();
        let limited = storage
            .put_part(
                limited_upload_id,
                &spec,
                chunked_body(0x5a, size, 4096),
                size / 2,
            )
            .await
            .expect_err("reject part above server limit");
        assert_eq!(limited.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(
            !fs::try_exists(storage.part_path(limited_upload_id, spec.index))
                .await
                .expect("check limited final part")
        );
        assert_no_temporary_parts(&storage, limited_upload_id).await;
    }

    #[tokio::test]
    async fn concurrent_identical_parts_are_idempotent() {
        let root = TestRoot::new();
        let storage = LocalStorage::new(root.path.clone())
            .await
            .expect("create test storage");
        let upload_id = Uuid::new_v4();
        let content = vec![0x31; 128 * 1024];
        let spec = part_spec(7, &content);
        let mut uploads = JoinSet::new();
        for _ in 0..16 {
            let storage = storage.clone();
            let spec = spec.clone();
            let content = content.clone();
            uploads.spawn(async move {
                storage
                    .put_part(upload_id, &spec, Body::from(content), 128 * 1024)
                    .await
            });
        }
        while let Some(result) = uploads.join_next().await {
            result
                .expect("join concurrent upload")
                .expect("identical upload succeeds");
        }

        assert_eq!(
            hash_file(&storage.part_path(upload_id, spec.index))
                .await
                .expect("hash concurrently stored part"),
            spec.blake3
        );
        assert_eq!(
            entry_names(&storage.upload_dir(upload_id)).await,
            vec![format!("{:08}.part", spec.index)]
        );
    }

    #[tokio::test]
    async fn concurrent_conflicting_parts_never_replace_the_winner() {
        let root = TestRoot::new();
        let storage = LocalStorage::new(root.path.clone())
            .await
            .expect("create test storage");
        let upload_id = Uuid::new_v4();
        let first_content = vec![0x41; 128 * 1024];
        let second_content = vec![0x42; 128 * 1024];
        let first_spec = part_spec(3, &first_content);
        let second_spec = part_spec(3, &second_content);

        let first = storage.put_part(
            upload_id,
            &first_spec,
            Body::from(first_content.clone()),
            first_content.len(),
        );
        let second = storage.put_part(
            upload_id,
            &second_spec,
            Body::from(second_content.clone()),
            second_content.len(),
        );
        let (first_result, second_result) = tokio::join!(first, second);
        let results = [first_result, second_result];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .filter(|error| error.status == StatusCode::CONFLICT)
                .count(),
            1
        );

        let final_path = storage.part_path(upload_id, first_spec.index);
        let winner = fs::read(&final_path).await.expect("read winning part");
        let (losing_spec, losing_content) = if winner == first_content {
            (&second_spec, second_content)
        } else {
            assert_eq!(winner, second_content);
            (&first_spec, first_content)
        };
        let retry = storage
            .put_part(
                upload_id,
                losing_spec,
                Body::from(losing_content),
                128 * 1024,
            )
            .await
            .expect_err("conflicting retry must not replace winner");
        assert_eq!(retry.status, StatusCode::CONFLICT);
        assert_eq!(
            fs::read(&final_path).await.expect("reread winning part"),
            winner
        );
        assert_eq!(
            entry_names(&storage.upload_dir(upload_id)).await,
            vec![format!("{:08}.part", first_spec.index)]
        );
    }

    #[tokio::test]
    async fn cancelling_a_streaming_upload_removes_its_temporary_file() {
        let root = TestRoot::new();
        let storage = LocalStorage::new(root.path.clone())
            .await
            .expect("create test storage");
        let upload_id = Uuid::new_v4();
        let spec = part_spec(9, &[0x61; 1024]);
        let (sender, receiver) = tokio::io::duplex(1024);
        let upload_storage = storage.clone();
        let upload = tokio::spawn(async move {
            upload_storage
                .put_part(
                    upload_id,
                    &spec,
                    Body::from_stream(ReaderStream::new(receiver)),
                    1024,
                )
                .await
        });

        let directory = storage.upload_dir(upload_id);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if entry_names(&directory)
                    .await
                    .iter()
                    .any(|name| name.ends_with(".tmp"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("streaming upload creates its temporary file");
        upload.abort();
        let _ = upload.await;
        drop(sender);
        assert_no_temporary_parts(&storage, upload_id).await;
        assert!(!fs::try_exists(storage.part_path(upload_id, 9))
            .await
            .expect("check cancelled final part"));
    }
}
