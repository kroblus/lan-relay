use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const PROTOCOL_VERSION: u32 = 2;
pub const OWNER_ID: &str = "owner";

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub name: String,
    pub max_file_bytes: u64,
    pub max_storage_bytes: u64,
    pub chunk_size: u64,
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            name: "我的电脑".into(),
            max_file_bytes: 2 * 1024 * 1024 * 1024,
            max_storage_bytes: 20 * 1024 * 1024 * 1024,
            chunk_size: 2 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub version: String,
    pub server_id: String,
    pub name: String,
    pub chunk_size: u64,
    pub max_file_bytes: u64,
    pub max_storage_bytes: u64,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen: i64,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invite {
    pub code: String,
    pub token: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairRequest {
    pub id: String,
    pub name: String,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairTicket {
    pub id: String,
    pub claim_token: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairClaim {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<Device>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NewPairRequest {
    pub name: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimRequest {
    pub id: String,
    pub claim_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewUpload {
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub sha256: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub build_id: Option<String>,
    pub request_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Upload {
    pub id: String,
    pub device_id: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub sha256: String,
    pub project: String,
    pub build_id: Option<String>,
    pub offset: u64,
    pub status: String,
    pub chunk_size: u64,
    pub created_at: i64,
    pub item_id: Option<String>,
    pub request_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApkMetadata {
    pub status: String,
    pub package_name: Option<String>,
    pub version_name: Option<String>,
    pub version_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub device_id: String,
    pub created_at: i64,
    pub project: String,
    pub text: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub sha256: String,
    pub build_id: Option<String>,
    pub attachment_ids: Vec<String>,
    pub note: String,
    pub test_status: String,
    pub apk: Option<ApkMetadata>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NewText {
    pub text: String,
    #[serde(default)]
    pub project: String,
    pub request_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NewFeedback {
    pub build_id: String,
    pub text: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub attachment_ids: Vec<String>,
    pub request_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PatchItem {
    pub project: Option<String>,
    pub note: Option<String>,
    pub test_status: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub hello: Hello,
    pub devices: Vec<Device>,
    pub pair_requests: Vec<PairRequest>,
    pub uploads: Vec<Upload>,
    pub items: Vec<Item>,
    pub used_bytes: u64,
    pub max_storage_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemPage {
    pub items: Vec<Item>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FileInfo {
    pub path: PathBuf,
    pub item: Item,
}
