use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use hmac::{Hmac, Mac};
use photo_backup_protocol::UploadPartSpec;
use rand::{rngs::OsRng, RngCore};
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid base64 key")]
    InvalidKey,
    #[error("encryption failed")]
    Encryption,
    #[error("part size must be greater than zero")]
    InvalidPartSize,
}

#[derive(Debug, Clone)]
pub struct EncryptedPart {
    pub spec: UploadPartSpec,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PreparedCrypto {
    pub plaintext_size: u64,
    pub dedup_token: String,
    pub wrapped_key: String,
    pub key_nonce: String,
    pub nonce_prefix: String,
    pub metadata_nonce: Option<String>,
    pub metadata_ciphertext: Option<String>,
    pub parts: Vec<EncryptedPart>,
}

pub fn prepare_file(
    source: &Path,
    output_dir: &Path,
    master_key_b64: &str,
    dedupe_key_b64: &str,
    part_size: usize,
    metadata_json: Option<&str>,
) -> Result<PreparedCrypto, CryptoError> {
    if part_size == 0 {
        return Err(CryptoError::InvalidPartSize);
    }

    let master_key = decode_key(master_key_b64)?;
    let dedupe_key = decode_key(dedupe_key_b64)?;
    fs::create_dir_all(output_dir)?;

    let mut plaintext_hasher = blake3::Hasher::new();
    let mut plaintext_size = 0_u64;
    let mut hash_reader = File::open(source)?;
    let mut hash_buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = hash_reader.read(&mut hash_buffer)?;
        if read == 0 {
            break;
        }
        plaintext_hasher.update(&hash_buffer[..read]);
        plaintext_size += read as u64;
    }

    let plaintext_hash = plaintext_hasher.finalize();
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&dedupe_key).map_err(|_| CryptoError::InvalidKey)?;
    mac.update(plaintext_hash.as_bytes());
    let dedup_token = hex::encode(mac.finalize().into_bytes());

    let mut asset_key = [0_u8; 32];
    let mut key_nonce = [0_u8; 24];
    let mut nonce_prefix = [0_u8; 16];
    OsRng.fill_bytes(&mut asset_key);
    OsRng.fill_bytes(&mut key_nonce);
    OsRng.fill_bytes(&mut nonce_prefix);

    let master_cipher = XChaCha20Poly1305::new((&master_key).into());
    let wrapped_key = master_cipher
        .encrypt(
            XNonce::from_slice(&key_nonce),
            Payload {
                msg: &asset_key,
                aad: b"photo-backup-wrapped-key-v1",
            },
        )
        .map_err(|_| CryptoError::Encryption)?;

    let asset_cipher = XChaCha20Poly1305::new((&asset_key).into());
    let (metadata_nonce, metadata_ciphertext) = if let Some(metadata) = metadata_json {
        let mut nonce = [0_u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let encrypted = asset_cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: metadata.as_bytes(),
                    aad: b"photo-backup-metadata-v1",
                },
            )
            .map_err(|_| CryptoError::Encryption)?;
        (
            Some(STANDARD.encode(nonce)),
            Some(STANDARD.encode(encrypted)),
        )
    } else {
        (None, None)
    };

    let mut source_reader = File::open(source)?;
    let mut parts = Vec::new();
    let mut index = 0_u32;
    loop {
        let mut plaintext = vec![0_u8; part_size];
        let mut filled = 0_usize;
        while filled < part_size {
            let read = source_reader.read(&mut plaintext[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled == 0 && index > 0 {
            break;
        }
        plaintext.truncate(filled);

        let mut nonce = [0_u8; 24];
        nonce[..16].copy_from_slice(&nonce_prefix);
        nonce[16..].copy_from_slice(&(index as u64).to_be_bytes());
        let mut aad = b"photo-backup-part-v1".to_vec();
        aad.extend_from_slice(&index.to_be_bytes());
        aad.extend_from_slice(&(filled as u64).to_be_bytes());

        let ciphertext = asset_cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::Encryption)?;
        let ciphertext_hash = blake3::hash(&ciphertext).to_hex().to_string();
        let part_path = output_dir.join(format!("{index:08}.part"));
        let mut part_file = File::create(&part_path)?;
        part_file.write_all(&ciphertext)?;
        part_file.sync_all()?;

        parts.push(EncryptedPart {
            spec: UploadPartSpec {
                index,
                ciphertext_size: ciphertext.len() as u64,
                ciphertext_blake3: ciphertext_hash,
            },
            path: part_path,
        });
        index += 1;

        if filled < part_size {
            break;
        }
    }

    Ok(PreparedCrypto {
        plaintext_size,
        dedup_token,
        wrapped_key: STANDARD.encode(wrapped_key),
        key_nonce: STANDARD.encode(key_nonce),
        nonce_prefix: STANDARD.encode(nonce_prefix),
        metadata_nonce,
        metadata_ciphertext,
        parts,
    })
}

fn decode_key(value: &str) -> Result<[u8; 32], CryptoError> {
    let decoded = STANDARD
        .decode(value)
        .map_err(|_| CryptoError::InvalidKey)?;
    decoded.try_into().map_err(|_| CryptoError::InvalidKey)
}
