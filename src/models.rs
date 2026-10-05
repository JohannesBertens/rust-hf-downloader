use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

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
    pub siblings: Vec<RepoFile>, // All files in the repo
    #[serde(default)]
    pub tags: Vec<String>,
    /// Top-level commit SHA of the repo (tree tip this metadata describes).
    /// Absent on some API shapes; authoritative pinning goes through
    /// [`crate::api::resolve_revision_sha`].
    #[serde(default)]
    #[allow(dead_code)]
    pub sha: Option<String>,
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
    pub rfilename: String, // API uses 'rfilename' for relative path
    #[serde(default)]
    pub size: Option<u64>,
    /// Git blob sha1 for non-LFS files, sha256 for LFS ones — the hub-cache
    /// blob name. Absent on plain siblings payloads.
    #[serde(default)]
    #[allow(dead_code)]
    pub oid: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub lfs: Option<LfsInfo>, // Reuse existing LfsInfo struct
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
    /// Git blob sha1 for non-LFS files, sha256 for LFS ones (tree entries).
    #[serde(default)]
    pub oid: Option<String>,
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
    pub files: Vec<QuantizationInfo>, // All files in this quantization type
    pub total_size: u64,
}

#[derive(Debug, Clone)]
pub struct ChunkProgress {
    pub chunk_id: usize,
    #[allow(dead_code)]
    pub start: u64,
    #[allow(dead_code)]
    pub end: u64,
    #[allow(dead_code)] // populated for future detail view; HUD uses the bitmap
    pub downloaded: u64,
    #[allow(dead_code)] // populated for future detail view; HUD uses the bitmap
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
    /// Total number of chunks the file was split into.
    /// `chunks` only ever contains *active* chunks (completed ones are
    /// removed); this field plus `chunk_completed` preserves the whole
    /// picture for monotonic progress display.
    pub num_chunks: usize,
    /// Per-chunk completion bitmap, indexed by chunk_id. Length == num_chunks.
    pub chunk_completed: Vec<bool>,
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
    /// Git revision (branch/tag/SHA) this entry was downloaded from.
    /// Absent in registries written before v2.7.0 — treated as `main`.
    #[serde(default)]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadRegistry {
    pub downloads: Vec<DownloadMetadata>,
}

/// Typed per-file result of a download attempt, collected by the engine's
/// download manager. The TUI ignores these (it renders from status strings);
/// the CLI maps them to events and exit codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOutcome {
    Complete { filename: String, bytes: u64 },
    AlreadyExists { filename: String, bytes: u64 },
    AuthRequired { model_id: String },
    Failed { filename: String, reason: String },
}

/// Typed verification result, reported through the engine's `verify_tx`
/// channel as each background verification finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    Ok {
        filename: String,
    },
    Mismatch {
        filename: String,
        expected_sha256: String,
        actual_sha256: String,
    },
    Error {
        filename: String,
        reason: String,
    },
    Missing {
        filename: String,
    },
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
    Gguf,     // Show quantizations (current behavior)
    Standard, // Show metadata + file tree
}

/// Combined download queue state to reduce lock complexity
#[derive(Debug, Default, Clone)]
pub struct QueueState {
    /// Number of downloads currently in queue
    pub size: usize,
    /// Total bytes of downloads in queue
    pub bytes: u64,
}

/// Summary of a file waiting in the download queue (for HUD display).
/// The transport itself is the `download_tx` channel; this mirrors the
/// queued items so the renderer can show names and sizes.
#[derive(Debug, Clone)]
pub struct QueueItemSummary {
    pub filename: String,
    pub total_size: u64,
}

impl QueueState {
    pub fn new(size: usize, bytes: u64) -> Self {
        Self { size, bytes }
    }

    pub fn add(&mut self, count: usize, bytes: u64) {
        self.size += count;
        self.bytes += bytes;
    }

    pub fn remove(&mut self, count: usize, bytes: u64) {
        self.size = self.size.saturating_sub(count);
        self.bytes = self.bytes.saturating_sub(bytes);
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
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
///
/// NOTE: `verified_bytes` is atomically updated by verification tasks.
/// Use `load(Ordering::Relaxed)` to read the current value.
/// This avoids lock contention while multiple files are verified concurrently.
#[derive(Debug, Clone)]
pub struct VerificationProgress {
    pub filename: String,
    pub verified_bytes: Arc<AtomicU64>,
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
    pub is_manual: bool, // True if triggered by 'v' key, false if automatic
}

// Default value for rate limit (50.0 MB/s)
fn default_rate_limit_mbps() -> f64 {
    50.0
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

    // Rate Limiting
    #[serde(default)]
    pub download_rate_limit_enabled: bool,
    #[serde(default = "default_rate_limit_mbps")]
    pub download_rate_limit_mbps: f64,

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
        let hf_token = std::env::var("HF_TOKEN").ok().filter(|s| !s.is_empty());
        Self {
            default_directory: crate::paths::default_download_dir()
                .to_string_lossy()
                .into_owned(),
            hf_token,
            concurrent_threads: 8,
            num_chunks: 20,
            min_chunk_size: 5 * 1024 * 1024,
            max_chunk_size: 100 * 1024 * 1024,
            max_retries: 5,
            download_timeout_secs: 300,
            retry_delay_secs: 1,
            progress_update_interval_ms: 200,
            download_rate_limit_enabled: false,
            download_rate_limit_mbps: 50.0,
            verification_on_completion: true,
            concurrent_verifications: 4,
            verification_buffer_size: 1024 * 1024,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_state_new_initializes_counts() {
        let state = QueueState::new(3, 1000);
        assert_eq!(state.size, 3);
        assert_eq!(state.bytes, 1000);
        assert!(!state.is_empty());
    }

    #[test]
    fn queue_state_add_accumulates() {
        let mut state = QueueState::new(3, 1000);
        state.add(2, 500);
        assert_eq!(state.size, 5);
        assert_eq!(state.bytes, 1500);
    }

    #[test]
    fn queue_state_remove_subtracts() {
        let mut state = QueueState::new(3, 1000);
        state.remove(1, 400);
        assert_eq!(state.size, 2);
        assert_eq!(state.bytes, 600);
    }

    #[test]
    fn queue_state_remove_saturates_at_zero() {
        // Documents actual behavior: removing more than was added saturates
        // both size and bytes at zero (no underflow panic).
        let mut state = QueueState::new(1, 100);
        state.remove(5, 999);
        assert_eq!(state.size, 0);
        assert_eq!(state.bytes, 0);
        assert!(state.is_empty());
    }

    #[test]
    fn queue_state_default_is_empty() {
        let state = QueueState::default();
        assert_eq!(state.size, 0);
        assert_eq!(state.bytes, 0);
        assert!(state.is_empty());
    }

    #[test]
    fn queue_state_is_empty_tracks_size_only() {
        // Documents actual behavior: emptiness is defined by `size` only,
        // even when `bytes` is non-zero.
        let state = QueueState::new(0, 100);
        assert!(state.is_empty());
    }

    // ---- serde parsing of API payload shapes ----

    #[test]
    fn model_metadata_parses_sha_sibling_oid_and_lfs() {
        let meta: ModelMetadata = serde_json::from_str(
            r#"{
                "id": "a/b",
                "sha": "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234",
                "siblings": [
                    {
                        "rfilename": "model.safetensors",
                        "size": 12345,
                        "oid": "6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b",
                        "lfs": {
                            "oid": "6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b",
                            "size": 12345,
                            "pointerSize": 134
                        }
                    },
                    {
                        "rfilename": "config.json",
                        "size": 100,
                        "oid": "d6a7702e2c35b4b1f9c8e3e9c2b1a0d4f7e6c5b4"
                    }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(meta.model_id, "a/b");
        assert_eq!(
            meta.sha.as_deref(),
            Some("f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234")
        );
        // LFS entry: top-level oid and lfs.oid both carry the sha256.
        let lfs_file = &meta.siblings[0];
        assert_eq!(
            lfs_file.oid.as_deref(),
            Some("6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b")
        );
        assert_eq!(
            lfs_file.lfs.as_ref().unwrap().oid,
            lfs_file.oid.clone().unwrap()
        );
        // Non-LFS entry: 40-hex git blob sha, no lfs block.
        assert_eq!(
            meta.siblings[1].oid.as_deref(),
            Some("d6a7702e2c35b4b1f9c8e3e9c2b1a0d4f7e6c5b4")
        );
        assert!(meta.siblings[1].lfs.is_none());
    }

    #[test]
    fn model_metadata_without_sha_or_oid_still_parses() {
        // Old fixtures / API shapes that predate sha and oid keep parsing:
        // every new field is #[serde(default)].
        let meta: ModelMetadata = serde_json::from_str(
            r#"{
                "id": "a/b",
                "siblings": [{"rfilename": "model.gguf", "size": 7}]
            }"#,
        )
        .unwrap();
        assert_eq!(meta.sha, None);
        assert_eq!(meta.siblings[0].rfilename, "model.gguf");
        assert_eq!(meta.siblings[0].oid, None);
        assert!(meta.siblings[0].lfs.is_none());
    }

    #[test]
    fn model_file_parses_tree_entry_oid_and_lfs() {
        let file: ModelFile = serde_json::from_str(
            r#"{
                "type": "file",
                "path": "sub/model.safetensors",
                "size": 12345,
                "oid": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "lfs": {
                    "oid": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "size": 12345,
                    "pointerSize": 134
                }
            }"#,
        )
        .unwrap();
        assert_eq!(file.file_type, "file");
        assert_eq!(file.path, "sub/model.safetensors");
        assert_eq!(
            file.oid.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert_eq!(file.lfs.as_ref().unwrap().size, 12345);
    }
}
