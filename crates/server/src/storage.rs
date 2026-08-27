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
        if bytes.len() as u64 != spec.ciphertext_size {
            return Err(AppError::bad_request("part size does not match manifest"));
        }
        if blake3::hash(&bytes).to_hex().as_str() != spec.ciphertext_blake3 {
            return Err(AppError::bad_request("part hash does not match manifest"));
        }
        let directory = self.upload_dir(upload_id);
        fs::create_dir_all(&directory).await?;
        let final_path = self.part_path(upload_id, spec.index);
        if fs::try_exists(&final_path).await? {
            let existing = fs::read(&final_path).await?;
            if existing.len() as u64 == spec.ciphertext_size
                && blake3::hash(&existing).to_hex().as_str() == spec.ciphertext_blake3
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

    pub async fn finalize(
        &self,
        account_id: Uuid,
        configured_path: &str,
        upload_id: Uuid,
        parts: &[UploadPartSpec],
    ) -> Result<(String, u64), AppError> {
        let account_dir = if configured_path.trim().is_empty() {
            self.root.join("blobs").join(account_id.to_string())
        } else {
            self.resolve_account_root(configured_path)?
        };
        fs::create_dir_all(&account_dir).await?;
        let final_path = account_dir.join(format!("{upload_id}.blob"));
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
