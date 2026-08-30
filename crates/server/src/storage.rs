use std::{
    future::poll_fn,
    path::{Path, PathBuf},
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

use crate::{error::AppError, rooted_fs::RootedFs};

#[derive(Debug, Clone)]
pub struct LocalStorage {
    files: RootedFs,
    #[cfg(all(test, target_os = "linux"))]
    root_path: PathBuf,
}

impl LocalStorage {
    pub async fn new(root: PathBuf) -> Result<Self, AppError> {
        fs::create_dir_all(&root).await?;
        let files = RootedFs::new(&root)?;
        files.ensure_dir(Path::new("uploads"))?;
        files.ensure_dir(Path::new("blobs"))?;
        Ok(Self {
            files,
            #[cfg(all(test, target_os = "linux"))]
            root_path: root,
        })
    }

    fn upload_dir(&self, upload_id: Uuid) -> PathBuf {
        PathBuf::from("uploads").join(upload_id.to_string())
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
        self.files.ensure_dir(&directory)?;
        let final_path = self.part_path(upload_id, spec.index);
        let temporary_path = directory.join(format!(
            ".{index:08}.{request_id}.tmp",
            index = spec.index,
            request_id = Uuid::new_v4()
        ));
        let (mut temporary, mut file) = TemporaryPart::create(self.files.clone(), temporary_path)?;
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
            cleanup_temporary_best_effort(&mut temporary);
            return Err(error);
        }

        match self.files.link_no_replace(temporary.path(), &final_path)? {
            true => {
                cleanup_temporary(&mut temporary)?;
                Ok(())
            }
            false => {
                let existing = existing_part_matches(&self.files, &final_path, spec).await;
                cleanup_temporary(&mut temporary)?;
                match existing? {
                    Some(true) => Ok(()),
                    Some(false) | None => Err(part_conflict()),
                }
            }
        }
    }

    pub async fn finalize(
        &self,
        configured_path: &str,
        upload_id: Uuid,
        parts: &[UploadPartSpec],
        filename: &str,
        expected_size: u64,
        expected_blake3: &str,
    ) -> Result<(String, u64), AppError> {
        let account_dir = StorageKey::parse(configured_path)?;
        self.files.ensure_dir(account_dir.as_path())?;
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
        let final_path = account_dir.join(&format!("{upload_id}.{extension}"));
        if let Some(matches) =
            existing_file_matches(&self.files, &final_path, expected_size, expected_blake3).await?
        {
            return if matches {
                Ok((path_to_key(&final_path)?, expected_size))
            } else {
                Err(AppError::conflict(
                    "stored blob conflicts with upload manifest",
                ))
            };
        }
        let temporary_path = account_dir.join(&format!(".{upload_id}.{}.tmp", Uuid::new_v4()));
        let (mut temporary, mut output) =
            TemporaryPart::create(self.files.clone(), temporary_path)?;
        let assembly = async {
            let mut total = 0_u64;
            for part in parts {
                let part_path = self.part_path(upload_id, part.index);
                let mut input = match self.files.open_read(&part_path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Err(AppError::conflict(format!(
                            "part {} is missing from storage",
                            part.index
                        )));
                    }
                    Err(error) => return Err(error.into()),
                };
                let copied = tokio::io::copy(&mut input, &mut output).await?;
                total = total.checked_add(copied).ok_or_else(|| {
                    AppError::conflict("assembled content size does not match manifest")
                })?;
            }
            output.sync_all().await?;
            Ok::<u64, AppError>(total)
        }
        .await;
        drop(output);
        let total = match assembly {
            Ok(total) => total,
            Err(error) => {
                cleanup_temporary_best_effort(&mut temporary);
                return Err(error);
            }
        };
        if total != expected_size {
            cleanup_temporary_best_effort(&mut temporary);
            return Err(AppError::conflict(
                "assembled content size does not match manifest",
            ));
        }
        let actual_hash = hash_file(&self.files, temporary.path()).await?;
        if actual_hash != expected_blake3 {
            cleanup_temporary_best_effort(&mut temporary);
            return Err(AppError::conflict(
                "assembled content hash does not match manifest",
            ));
        }
        let published = self.files.link_no_replace(temporary.path(), &final_path)?;
        cleanup_temporary(&mut temporary)?;
        if !published
            && !existing_file_matches(&self.files, &final_path, expected_size, expected_blake3)
                .await?
                .unwrap_or(false)
        {
            return Err(AppError::conflict(
                "stored blob conflicts with upload manifest",
            ));
        }
        Ok((path_to_key(&final_path)?, total))
    }

    pub async fn open_blob(
        &self,
        configured_account_path: &str,
        storage_path: &str,
    ) -> Result<fs::File, AppError> {
        let key = scoped_blob_key(configured_account_path, storage_path)?;
        Ok(self.files.open_read(key.as_path())?)
    }

    pub async fn remove_blob(
        &self,
        configured_account_path: &str,
        storage_path: &str,
    ) -> Result<(), AppError> {
        let key = scoped_blob_key(configured_account_path, storage_path)?;
        self.files.remove_file(key.as_path())?;
        Ok(())
    }

    pub async fn validate_account_path(&self, configured_path: &str) -> Result<(), AppError> {
        let directory = StorageKey::parse(configured_path)?;
        if directory.first_component() == Some("uploads") {
            return Err(AppError::bad_request(
                "storage_path uses a reserved server directory",
            ));
        }
        self.files.ensure_dir(directory.as_path())?;
        let probe = directory.join(&format!(".photo-backup-write-test-{}", Uuid::new_v4()));
        let (mut temporary, mut file) = TemporaryPart::create(self.files.clone(), probe)?;
        file.write_all(b"ok").await?;
        file.sync_all().await?;
        drop(file);
        cleanup_temporary(&mut temporary)?;
        Ok(())
    }

    pub fn account_paths_overlap(&self, first: &str, second: &str) -> Result<bool, AppError> {
        let first = StorageKey::parse(first)?;
        let second = StorageKey::parse(second)?;
        Ok(first.0 == second.0
            || first.is_strict_descendant_of(&second)
            || second.is_strict_descendant_of(&first))
    }

    /// Exercise the same local create/write/fsync/remove path required by uploads.
    pub async fn probe_readiness(&self) -> Result<(), AppError> {
        let probe = PathBuf::from(format!(".photo-backup-readiness-{}", Uuid::new_v4()));
        let (mut temporary, mut file) = TemporaryPart::create(self.files.clone(), probe)?;
        let operation = async {
            file.write_all(b"photo-backup-readiness-v1").await?;
            file.sync_all().await?;
            Ok::<(), AppError>(())
        }
        .await;
        drop(file);
        let cleanup = cleanup_temporary(&mut temporary);
        operation?;
        cleanup?;
        Ok(())
    }

    #[cfg(all(test, target_os = "linux"))]
    fn test_path(&self, relative: &Path) -> PathBuf {
        self.root_path.join(relative)
    }

    #[cfg(all(test, target_os = "linux"))]
    fn inject_before_rooted_operation_once(&self, hook: impl FnOnce() + Send + 'static) {
        self.files.inject_before_operation_once(hook);
    }
}

async fn hash_file(files: &RootedFs, path: &Path) -> Result<String, AppError> {
    let mut file = files.open_read(path)?;
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
    files: &RootedFs,
    path: &Path,
    spec: &UploadPartSpec,
) -> Result<Option<bool>, AppError> {
    existing_file_matches(files, path, spec.size, &spec.blake3).await
}

async fn existing_file_matches(
    files: &RootedFs,
    path: &Path,
    expected_size: u64,
    expected_blake3: &str,
) -> Result<Option<bool>, AppError> {
    let mut file = match files.open_read(path) {
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
        if total > expected_size {
            return Ok(Some(false));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(Some(
        total == expected_size && hasher.finalize().to_hex().as_str() == expected_blake3,
    ))
}

fn part_conflict() -> AppError {
    AppError::conflict("stored part conflicts with upload manifest")
}

struct TemporaryPart {
    files: RootedFs,
    path: PathBuf,
    remove_on_drop: bool,
}

impl TemporaryPart {
    fn create(files: RootedFs, path: PathBuf) -> Result<(Self, fs::File), AppError> {
        let file = files.create_new(&path)?;
        Ok((
            Self {
                files,
                path,
                remove_on_drop: true,
            },
            file,
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn remove(&mut self) -> Result<(), AppError> {
        self.files.remove_file(&self.path)?;
        self.remove_on_drop = false;
        Ok(())
    }
}

impl Drop for TemporaryPart {
    fn drop(&mut self) {
        if self.remove_on_drop {
            if let Err(error) = self.files.remove_file(&self.path) {
                tracing::warn!(
                    ?error,
                    path = %self.path.display(),
                    "failed to clean temporary upload part"
                );
            }
        }
    }
}

fn cleanup_temporary(temporary: &mut TemporaryPart) -> Result<(), AppError> {
    temporary.remove()
}

fn cleanup_temporary_best_effort(temporary: &mut TemporaryPart) {
    if let Err(error) = cleanup_temporary(temporary) {
        tracing::warn!(?error, "failed to durably clean temporary upload part");
    }
}

#[derive(Debug, Clone)]
struct StorageKey(String);

impl StorageKey {
    fn parse(raw: &str) -> Result<Self, AppError> {
        if raw.is_empty()
            || raw.len() > 4096
            || raw.trim() != raw
            || raw.starts_with('/')
            || raw.contains(['\\', ':', '\0'])
            || raw.chars().any(char::is_control)
        {
            return Err(invalid_storage_key());
        }
        for component in raw.split('/') {
            if component.is_empty()
                || component == "."
                || component == ".."
                || component.starts_with('.')
                || component.trim() != component
            {
                return Err(invalid_storage_key());
            }
        }
        let path = Path::new(raw);
        if path.is_absolute() {
            return Err(invalid_storage_key());
        }
        Ok(Self(raw.to_owned()))
    }

    fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    fn join(&self, component: &str) -> PathBuf {
        self.as_path().join(component)
    }

    fn first_component(&self) -> Option<&str> {
        self.0.split('/').next()
    }

    fn is_strict_descendant_of(&self, parent: &Self) -> bool {
        self.as_path()
            .strip_prefix(parent.as_path())
            .is_ok_and(|relative| !relative.as_os_str().is_empty())
    }
}

fn scoped_blob_key(account_path: &str, storage_path: &str) -> Result<StorageKey, AppError> {
    let account = StorageKey::parse(account_path)?;
    let storage = StorageKey::parse(storage_path)?;
    if !storage.is_strict_descendant_of(&account) {
        return Err(AppError::bad_request(
            "blob key is outside the account storage directory",
        ));
    }
    Ok(storage)
}

fn path_to_key(path: &Path) -> Result<String, AppError> {
    let value = path.to_str().ok_or_else(invalid_storage_key)?.to_owned();
    StorageKey::parse(&value)?;
    Ok(value)
}

fn invalid_storage_key() -> AppError {
    AppError::bad_request("storage key must use normal relative components under DATA_DIR")
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{os::unix::fs::symlink, time::Duration};
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
        let names = entry_names(&storage.test_path(&storage.upload_dir(upload_id))).await;
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
            fs::metadata(storage.test_path(&storage.part_path(upload_id, spec.index)))
                .await
                .expect("read stored part metadata")
                .len(),
            size as u64
        );
        assert_eq!(
            hash_file(&storage.files, &storage.part_path(upload_id, spec.index),)
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
        assert!(!fs::try_exists(
            storage.test_path(&storage.part_path(rejected_upload_id, spec.index))
        )
        .await
        .expect("check rejected final part"));
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
        assert!(!fs::try_exists(
            storage.test_path(&storage.part_path(limited_upload_id, spec.index))
        )
        .await
        .expect("check limited final part"));
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
            hash_file(&storage.files, &storage.part_path(upload_id, spec.index),)
                .await
                .expect("hash concurrently stored part"),
            spec.blake3
        );
        assert_eq!(
            entry_names(&storage.test_path(&storage.upload_dir(upload_id))).await,
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

        let final_key = storage.part_path(upload_id, first_spec.index);
        let final_path = storage.test_path(&final_key);
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
            entry_names(&storage.test_path(&storage.upload_dir(upload_id))).await,
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

        let directory = storage.test_path(&storage.upload_dir(upload_id));
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
        assert!(
            !fs::try_exists(storage.test_path(&storage.part_path(upload_id, 9)))
                .await
                .expect("check cancelled final part")
        );
    }

    #[tokio::test]
    async fn rejects_non_relative_and_special_storage_keys() {
        let root = TestRoot::new();
        let storage = LocalStorage::new(root.path.clone())
            .await
            .expect("create test storage");
        for invalid in [
            "/tmp/outside",
            "../outside",
            "blobs/../outside",
            "blobs/./account",
            "blobs//account",
            "blobs/account/",
            "C:/outside",
            "blobs\\outside",
            "blobs/.hidden",
        ] {
            let error = storage
                .validate_account_path(invalid)
                .await
                .expect_err("reject invalid storage key");
            assert_eq!(error.status, StatusCode::BAD_REQUEST, "key {invalid:?}");
        }
        storage
            .validate_account_path("blobs/account-a")
            .await
            .expect("accept normal relative account directory");
        for invalid_object in [
            "/tmp/outside.bin",
            "blobs/account-a/../outside.bin",
            "blobs/account-a//outside.bin",
        ] {
            let open_error = storage
                .open_blob("blobs/account-a", invalid_object)
                .await
                .expect_err("reject invalid object key on open");
            assert_eq!(open_error.status, StatusCode::BAD_REQUEST);
            let remove_error = storage
                .remove_blob("blobs/account-a", invalid_object)
                .await
                .expect_err("reject invalid object key on remove");
            assert_eq!(remove_error.status, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn symlinks_and_parent_swaps_never_reach_outside_data_dir() {
        let root = TestRoot::new();
        let outside = TestRoot::new();
        let storage = LocalStorage::new(root.path.clone())
            .await
            .expect("create test storage");
        storage
            .validate_account_path("blobs/account-a")
            .await
            .expect("create account directory");
        std::fs::create_dir_all(&outside.path).expect("create outside directory");
        std::fs::write(outside.path.join("secret.bin"), b"outside-secret")
            .expect("write outside secret");
        symlink(&outside.path, root.path.join("blobs/linked-account"))
            .expect("create account directory symlink");
        assert!(
            storage
                .validate_account_path("blobs/linked-account")
                .await
                .is_err(),
            "account creation followed a directory symlink"
        );
        symlink(
            outside.path.join("secret.bin"),
            root.path.join("blobs/account-a/direct-link"),
        )
        .expect("create direct object symlink");

        assert!(
            storage
                .open_blob("blobs/account-a", "blobs/account-a/direct-link")
                .await
                .is_err(),
            "direct blob symlink was opened"
        );
        assert!(
            storage
                .remove_blob("blobs/account-a", "blobs/account-a/direct-link")
                .await
                .is_err(),
            "direct blob symlink was accepted for deletion"
        );
        assert_eq!(
            std::fs::read(outside.path.join("secret.bin")).expect("read outside secret"),
            b"outside-secret"
        );

        let original_account = root.path.join("blobs/account-a");
        let displaced_account = root.path.join("blobs/account-a-displaced");
        std::fs::write(original_account.join("inside.bin"), b"inside")
            .expect("write original account object");
        let outside_for_swap = outside.path.clone();
        storage.inject_before_rooted_operation_once(move || {
            std::fs::rename(&original_account, &displaced_account)
                .expect("displace account directory");
            symlink(&outside_for_swap, &original_account)
                .expect("swap account for outside symlink");
        });
        assert!(
            storage
                .open_blob("blobs/account-a", "blobs/account-a/secret.bin")
                .await
                .is_err(),
            "symlink swap escaped DATA_DIR"
        );
        assert_eq!(
            std::fs::read(outside.path.join("secret.bin")).expect("reread outside secret"),
            b"outside-secret"
        );

        storage
            .validate_account_path("blobs/account-b")
            .await
            .expect("create second account directory");
        let second_account = root.path.join("blobs/account-b");
        let displaced_second = root.path.join("blobs/account-b-displaced");
        std::fs::write(second_account.join("secret.bin"), b"inside-delete")
            .expect("write second account object");
        let outside_for_delete = outside.path.clone();
        storage.inject_before_rooted_operation_once(move || {
            std::fs::rename(&second_account, &displaced_second)
                .expect("displace second account directory");
            symlink(&outside_for_delete, &second_account)
                .expect("swap second account for outside symlink");
        });
        assert!(
            storage
                .remove_blob("blobs/account-b", "blobs/account-b/secret.bin")
                .await
                .is_err(),
            "delete followed a swapped account directory"
        );
        assert_eq!(
            std::fs::read(outside.path.join("secret.bin"))
                .expect("outside secret survives delete swap"),
            b"outside-secret"
        );
    }

    #[tokio::test]
    async fn cross_account_delete_is_component_scoped() {
        let root = TestRoot::new();
        let storage = LocalStorage::new(root.path.clone())
            .await
            .expect("create test storage");
        storage
            .validate_account_path("blobs/account")
            .await
            .expect("create first account directory");
        storage
            .validate_account_path("blobs/account-other")
            .await
            .expect("create second account directory");
        let other_blob = root.path.join("blobs/account-other/blob.bin");
        std::fs::write(&other_blob, b"other-account").expect("write other account blob");

        let error = storage
            .remove_blob("blobs/account", "blobs/account-other/blob.bin")
            .await
            .expect_err("cross-account delete must fail");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(
            storage
                .open_blob("blobs/account", "blobs/account-other/blob.bin")
                .await
                .is_err(),
            "cross-account read must fail"
        );
        assert_eq!(
            std::fs::read(other_blob).expect("other account blob survives"),
            b"other-account"
        );
    }

    #[tokio::test]
    async fn normal_relative_blob_round_trip_uses_object_keys() {
        let root = TestRoot::new();
        let storage = LocalStorage::new(root.path.clone())
            .await
            .expect("create test storage");
        let account_path = "blobs/account-a";
        storage
            .validate_account_path(account_path)
            .await
            .expect("create account directory");
        let upload_id = Uuid::new_v4();
        let content = b"rooted storage round trip";
        let spec = part_spec(0, content);
        storage
            .put_part(
                upload_id,
                &spec,
                Body::from(content.as_slice()),
                content.len(),
            )
            .await
            .expect("write rooted part");
        let (object_key, stored_size) = storage
            .finalize(
                account_path,
                upload_id,
                std::slice::from_ref(&spec),
                "photo.jpg",
                content.len() as u64,
                &spec.blake3,
            )
            .await
            .expect("finalize rooted blob");
        assert!(object_key.starts_with("blobs/account-a/"));
        assert!(!Path::new(&object_key).is_absolute());
        assert_eq!(stored_size, content.len() as u64);

        let mut blob = storage
            .open_blob(account_path, &object_key)
            .await
            .expect("open rooted blob");
        let mut restored = Vec::new();
        blob.read_to_end(&mut restored)
            .await
            .expect("read rooted blob");
        assert_eq!(restored, content);
        storage
            .remove_blob(account_path, &object_key)
            .await
            .expect("remove rooted blob");
        assert!(
            storage.open_blob(account_path, &object_key).await.is_err(),
            "removed blob remained readable"
        );
    }
}
