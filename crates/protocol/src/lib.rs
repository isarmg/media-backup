use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const API_VERSION: &str = "v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapRequest {
    pub username: String,
    pub password: String,
    pub device_name: String,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapResponse {
    pub account_id: Uuid,
    pub device_id: Uuid,
    pub bearer_token: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Photo,
    Video,
    Other,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Photo => "photo",
            Self::Video => "video",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadPartSpec {
    pub index: u32,
    pub ciphertext_size: u64,
    pub ciphertext_blake3: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUploadRequest {
    pub source_asset_id: String,
    pub source_resource_id: String,
    pub media_kind: MediaKind,
    pub role: String,
    pub filename: String,
    pub mime_type: String,
    pub source_created_at_ms: i64,
    pub plaintext_size: u64,
    pub dedup_token: String,
    pub wrapped_key: String,
    pub key_nonce: String,
    pub nonce_prefix: String,
    pub metadata_nonce: Option<String>,
    pub metadata_ciphertext: Option<String>,
    pub parts: Vec<UploadPartSpec>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UploadDisposition {
    Upload,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUploadResponse {
    pub disposition: UploadDisposition,
    pub upload_id: Option<Uuid>,
    pub resource_id: Option<Uuid>,
    pub missing_parts: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadStatusResponse {
    pub upload_id: Uuid,
    pub state: String,
    pub missing_parts: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteUploadResponse {
    pub resource_id: Uuid,
    pub deduplicated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceManifest {
    pub resource_id: Uuid,
    pub asset_id: Uuid,
    pub source_asset_id: String,
    pub source_resource_id: String,
    pub media_kind: MediaKind,
    pub role: String,
    pub filename: String,
    pub mime_type: String,
    pub source_created_at_ms: i64,
    pub plaintext_size: u64,
    pub ciphertext_size: u64,
    pub wrapped_key: String,
    pub key_nonce: String,
    pub nonce_prefix: String,
    pub metadata_nonce: Option<String>,
    pub metadata_ciphertext: Option<String>,
    pub parts: Vec<UploadPartSpec>,
    pub content_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}
