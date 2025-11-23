use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelInfo {
    #[serde(rename = "modelId")]
    pub id: String,
    pub author: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub likes: u64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(rename = "lastModified", default)]
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LfsInfo {
    pub oid: String,
    pub size: u64,
    #[serde(rename = "pointerSize")]
    pub pointer_size: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    #[serde(rename = "type")]
    pub file_type: String,
    pub path: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub lfs: Option<LfsInfo>,
}

#[derive(Debug, Clone)]
pub struct QuantizationInfo {
    pub quant_type: String,
    pub filename: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChunkProgress {
    pub chunk_id: usize,
    #[allow(dead_code)]
    pub start: u64,
    #[allow(dead_code)]
    pub end: u64,
    pub downloaded: u64,
    pub total: u64,
    pub speed_mbps: f64,
    pub is_active: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DownloadProgress {
    pub model_id: String,
    pub filename: String,
    pub downloaded: u64,
    pub total: u64,
    pub speed_mbps: f64,
    pub chunks: Vec<ChunkProgress>,
    pub verifying: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DownloadStatus {
    Incomplete,
    Complete,
    HashMismatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadMetadata {
    pub model_id: String,
    pub filename: String,
    pub url: String,
    pub local_path: String,
    pub total_size: u64,
    pub downloaded_size: u64,
    pub status: DownloadStatus,
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadRegistry {
    pub downloads: Vec<DownloadMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupMode {
    None,
    DownloadPath,
    ResumeDownload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Editing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Models,
    Quantizations,
}

pub type QuantizationCache = HashMap<String, Vec<QuantizationInfo>>;
pub type CompleteDownloads = HashMap<String, DownloadMetadata>;

/// Progress tracking for an active verification operation
#[derive(Debug, Clone)]
pub struct VerificationProgress {
    pub filename: String,
    #[allow(dead_code)]
    pub local_path: String,
    pub verified_bytes: u64,
    pub total_bytes: u64,
    pub speed_mbps: f64,
}

/// Item in the verification queue
#[derive(Debug, Clone)]
pub struct VerificationQueueItem {
    pub filename: String,
    pub local_path: String,
    pub expected_sha256: String,
    pub total_size: u64,
    #[allow(dead_code)]
    pub is_manual: bool,  // True if triggered by 'v' key, false if automatic
}
