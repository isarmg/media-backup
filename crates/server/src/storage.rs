use std::path::{Component, Path, PathBuf};

use bytes::Bytes;
use photo_backup_protocol::UploadPartSpec;
use tokio::{fs, io::AsyncWriteExt};
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
        bytes: Bytes,
    ) -> Result<(), AppError> {
        if bytes.len() as u64 != spec.size {
            return Err(AppError::bad_request("part size does not match manifest"));
        }
        if blake3::hash(&bytes).to_hex().as_str() != spec.blake3 {
            return Err(AppError::bad_request("part hash does not match manifest"));
        }
        let directory = self.upload_dir(upload_id);
        fs::create_dir_all(&directory).await?;
        let final_path = self.part_path(upload_id, spec.index);
        if fs::try_exists(&final_path).await? {
            let existing = fs::read(&final_path).await?;
            if existing.len() as u64 == spec.size
                && blake3::hash(&existing).to_hex().as_str() == spec.blake3
            {
                return Ok(());
            }
        }
        let temporary = directory.join(format!("{index:08}.tmp", index = spec.index));
        let mut file = fs::File::create(&temporary).await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        if fs::try_exists(&final_path).await? {
            fs::remove_file(&final_path).await?;
        }
        fs::rename(temporary, final_path).await?;
        Ok(())
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
    use tokio::io::AsyncReadExt;

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
