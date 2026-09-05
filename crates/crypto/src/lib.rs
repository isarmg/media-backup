use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use media_backup_protocol::UploadPartSpec;
use thiserror::Error;

pub const MAX_PART_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_CONTENT_PARTS: usize = 4096;

#[derive(Debug, Error)]
pub enum ContentError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("part size must be between 1 byte and 64 MiB")]
    InvalidPartSize,
    #[error("content exceeds the 4096-part budget")]
    TooManyParts,
    #[error("content size or BLAKE3 hash does not match")]
    Integrity,
}

#[derive(Debug, Clone)]
pub struct PreparedPart {
    pub spec: UploadPartSpec,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PreparedContent {
    pub content_size: u64,
    pub content_blake3: String,
    pub parts: Vec<PreparedPart>,
}

/// Split an original media file into resumable plaintext parts.
///
/// Confidentiality is provided by HTTPS while the parts are transmitted. The
/// bytes written here and reconstructed by the server are intentionally the
/// original media bytes so server-side browsing and processing remain possible.
pub fn prepare_file(
    source: &Path,
    output_dir: &Path,
    part_size: usize,
) -> Result<PreparedContent, ContentError> {
    if part_size == 0 || part_size > MAX_PART_BYTES {
        return Err(ContentError::InvalidPartSize);
    }
    let mut reader = File::open(source)?;
    if reader.metadata()?.len().div_ceil(part_size as u64).max(1) > MAX_CONTENT_PARTS as u64 {
        return Err(ContentError::TooManyParts);
    }
    fs::create_dir_all(output_dir)?;
    let mut content_hasher = blake3::Hasher::new();
    let mut content_size = 0_u64;
    let mut parts = Vec::new();
    let mut index = 0_u32;

    loop {
        if parts.len() == MAX_CONTENT_PARTS {
            let mut probe = [0];
            if reader.read(&mut probe)? != 0 {
                return Err(ContentError::TooManyParts);
            }
            break;
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(part_size)
            .map_err(|_| std::io::Error::other("part allocation refused"))?;
        bytes.resize(part_size, 0_u8);
        let mut filled = 0_usize;
        while filled < part_size {
            let read = reader.read(&mut bytes[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled == 0 && index > 0 {
            break;
        }
        bytes.truncate(filled);
        content_hasher.update(&bytes);
        content_size += filled as u64;
        parts
            .try_reserve(1)
            .map_err(|_| std::io::Error::other("part metadata allocation refused"))?;
        let path = output_dir.join(format!("{index:08}.part"));
        let mut output = File::create(&path)?;
        output.write_all(&bytes)?;
        output.sync_all()?;
        parts.push(PreparedPart {
            spec: UploadPartSpec {
                index,
                size: filled as u64,
                blake3: blake3::hash(&bytes).to_hex().to_string(),
            },
            path,
        });
        index += 1;
        if filled < part_size {
            break;
        }
    }

    Ok(PreparedContent {
        content_size,
        content_blake3: content_hasher.finalize().to_hex().to_string(),
        parts,
    })
}

/// Verify a downloaded plaintext object and publish it atomically at `output`.
pub fn restore_file(
    downloaded: &Path,
    output: &Path,
    expected_size: u64,
    expected_blake3: &str,
) -> Result<(), ContentError> {
    let mut input = File::open(downloaded)?;
    let temporary = output.with_extension("media-backup.tmp");
    let operation = (|| {
        let mut restored = File::create(&temporary)?;
        let mut hasher = blake3::Hasher::new();
        let mut size = 0_u64;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            restored.write_all(&buffer[..read])?;
            size += read as u64;
        }
        if size != expected_size || hasher.finalize().to_hex().as_str() != expected_blake3 {
            return Err(ContentError::Integrity);
        }
        restored.sync_all()?;
        Ok(())
    })();
    if operation.is_ok() {
        fs::rename(&temporary, output)?;
    } else {
        let _ = fs::remove_file(&temporary);
    }
    operation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_budgets_fail_before_creating_output() {
        let root = temporary("prepare-budgets");
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        let output = root.join("parts");
        for size in [0, MAX_PART_BYTES + 1, usize::MAX] {
            assert!(matches!(
                prepare_file(&source, &output, size),
                Err(ContentError::InvalidPartSize)
            ));
            assert!(!output.exists());
        }
        fs::write(&source, vec![1; MAX_CONTENT_PARTS + 1]).unwrap();
        assert!(matches!(
            prepare_file(&source, &output, 1),
            Err(ContentError::TooManyParts)
        ));
        assert!(!output.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn temporary(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "media-backup-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn plaintext_parts_and_restore_round_trip() {
        let source = temporary("source");
        let parts_dir = temporary("parts");
        let downloaded = temporary("downloaded");
        let restored = temporary("restored");
        let bytes = b"original media remains plaintext on the server";
        fs::write(&source, bytes).unwrap();
        let prepared = prepare_file(&source, &parts_dir, 7).unwrap();
        let mut object = File::create(&downloaded).unwrap();
        for part in &prepared.parts {
            object.write_all(&fs::read(&part.path).unwrap()).unwrap();
        }
        drop(object);
        restore_file(
            &downloaded,
            &restored,
            prepared.content_size,
            &prepared.content_blake3,
        )
        .unwrap();
        assert_eq!(fs::read(&restored).unwrap(), bytes);
        let _ = fs::remove_file(source);
        let _ = fs::remove_file(downloaded);
        let _ = fs::remove_file(restored);
        let _ = fs::remove_dir_all(parts_dir);
    }

    #[test]
    fn restore_rejects_modified_download() {
        let downloaded = temporary("modified");
        let restored = temporary("rejected");
        fs::write(&downloaded, b"modified").unwrap();
        assert!(matches!(
            restore_file(
                &downloaded,
                &restored,
                8,
                &blake3::hash(b"expected").to_hex()
            ),
            Err(ContentError::Integrity)
        ));
        assert!(!restored.exists());
        let _ = fs::remove_file(downloaded);
    }
}
