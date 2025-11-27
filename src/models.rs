use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelInfo {
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


/// Extended model metadata from /api/models/{model_id}
#[derive(Debug, Clone, Deserialize)]
pub struct ModelMetadata {
    #[serde(rename = "id")]
    pub model_id: String,
    #[serde(default)]
    pub library_name: Option<String>,
    #[serde(default)]
    pub pipeline_tag: Option<String>,
    #[serde(default)]
    pub card_data: Option<ModelCardData>,
    #[serde(default)]
    pub siblings: Vec<RepoFile>,  // All files in the repo
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelCardData {
    #[serde(default)]
    pub base_model: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub language: Option<Vec<String>>,
    #[serde(default)]
    #[allow(dead_code)]
    pub datasets: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoFile {
    pub rfilename: String,  // API uses 'rfilename' for relative path
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub lfs: Option<LfsInfo>,  // Reuse existing LfsInfo struct
}

/// Tree node for hierarchical file display
#[derive(Debug, Clone)]
pub struct FileTreeNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub children: Vec<FileTreeNode>,
    pub expanded: bool,
    pub depth: usize,
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
pub struct QuantizationGroup {
    pub quant_type: String,
    pub files: Vec<QuantizationInfo>,  // All files in this quantization type
    pub total_size: u64,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupMode {
    None,
    DownloadPath,
    ResumeDownload,
    Options,
    AuthError { model_url: String },
    SearchPopup,
}

/// Filter presets for quick filter combinations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterPreset {
    NoFilters,
    Popular,
    HighlyRated,
    Recent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    #[allow(dead_code)]  // Kept for potential future use (inline editing)
    Editing,
}

/// Sort field options for model search
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SortField {
    #[default]
    Downloads,
    Likes,
    Modified,
    Name,
}

/// Sort direction (ascending or descending)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SortDirection {
    Ascending,
    #[default]
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Models,
    QuantizationGroups,
    QuantizationFiles,
    ModelMetadata,
    FileTree,
}

/// Model display mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelDisplayMode {
    Gguf,      // Show quantizations (current behavior)
    Standard,  // Show metadata + file tree
}

pub type QuantizationCache = HashMap<String, Vec<QuantizationGroup>>;
pub type CompleteDownloads = HashMap<String, DownloadMetadata>;

// Additional cache types for comprehensive API caching
pub type MetadataCache = HashMap<String, ModelMetadata>;
pub type FileTreeCache = HashMap<String, FileTreeNode>;
pub type SearchCache = HashMap<SearchKey, Vec<ModelInfo>>;

/// Search cache key that includes all filter parameters
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SearchKey {
    pub query: String,
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    pub min_downloads: u64,
    pub min_likes: u64,
}

/// Unified API cache container for all cached data
#[derive(Debug, Default)]
pub struct ApiCache {
    pub metadata: MetadataCache,
    pub quantizations: QuantizationCache,
    pub file_trees: FileTreeCache,
    pub searches: SearchCache,
}

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

/// Application options/settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppOptions {
    // General
    pub default_directory: String,
    pub hf_token: Option<String>,
    
    // Download Settings
    pub concurrent_threads: usize,
    pub num_chunks: usize,
    pub min_chunk_size: u64,
    pub max_chunk_size: u64,
    pub max_retries: u32,
    pub download_timeout_secs: u64,
    pub retry_delay_secs: u64,
    pub progress_update_interval_ms: u64,
    
    // Verification Settings
    pub verification_on_completion: bool,
    pub concurrent_verifications: usize,
    pub verification_buffer_size: usize,
    pub verification_update_interval: usize,
    
    // UI State (not serialized)
    #[serde(skip)]
    pub selected_field: usize,
    #[serde(skip)]
    pub editing_directory: bool,
    #[serde(skip)]
    pub editing_token: bool,
    
    // Filter & Sort Settings (NEW)
    #[serde(default)]
    pub default_sort_field: SortField,
    #[serde(default)]
    pub default_sort_direction: SortDirection,
    #[serde(default)]
    pub default_min_downloads: u64,
    #[serde(default)]
    pub default_min_likes: u64,
}

impl Default for AppOptions {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let hf_token = std::env::var("HF_TOKEN").ok().filter(|s| !s.is_empty());
        Self {
            default_directory: format!("{}/models", home),
            hf_token,
            concurrent_threads: 8,
            num_chunks: 20,
            min_chunk_size: 5 * 1024 * 1024,
            max_chunk_size: 100 * 1024 * 1024,
            max_retries: 5,
            download_timeout_secs: 300,
            retry_delay_secs: 1,
            progress_update_interval_ms: 200,
            verification_on_completion: true,
            concurrent_verifications: 2,
            verification_buffer_size: 128 * 1024,
            verification_update_interval: 100,
            selected_field: 0,
            editing_directory: false,
            editing_token: false,
            // Filter & Sort defaults
            default_sort_field: SortField::Downloads,
            default_sort_direction: SortDirection::Descending,
            default_min_downloads: 0,
            default_min_likes: 0,
        }
    }
}
