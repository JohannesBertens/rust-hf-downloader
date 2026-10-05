//! Stable, additive-only NDJSON event schema (snapshot-tested).

use serde::Serialize;

use super::resolve::FileSpec;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FileDto {
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
}

impl From<&FileSpec> for FileDto {
    fn from(f: &FileSpec) -> Self {
        Self {
            filename: f.filename.clone(),
            size_bytes: f.size_bytes,
            sha256: f.sha256.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Summary {
    pub files: usize,
    pub downloaded: usize,
    pub skipped: usize,
    pub verified: usize,
    pub failed: usize,
    pub hash_mismatch: usize,
    pub total_bytes: u64,
}

/// Aggregate run progress for multi-file runs (engine downloads serially,
/// so the active file's speed is the aggregate speed). Omitted on
/// single-file runs and in JSON when absent.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OverallProgress {
    /// Files fully processed (downloaded + skipped + failed).
    pub files_done: usize,
    pub files_total: usize,
    /// Bytes of finished files + the active file's partial bytes.
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Resolved {
        model: String,
        files: Vec<FileDto>,
        total_bytes: u64,
    },
    DownloadStart {
        filename: String,
        index: usize,
        count: usize,
        size_bytes: u64,
    },
    Progress {
        filename: String,
        downloaded_bytes: u64,
        total_bytes: u64,
        speed_mbps: f64,
        percent: f64,
        /// Present when the run covers multiple files.
        #[serde(skip_serializing_if = "Option::is_none")]
        overall: Option<OverallProgress>,
    },
    FileComplete {
        filename: String,
        status: &'static str, // "downloaded" | "already_exists"
        bytes: u64,
    },
    VerificationStart {
        filename: String,
    },
    VerificationResult {
        filename: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_sha256: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actual_sha256: Option<String>,
    },
    Done {
        summary: Summary,
    },
    /// `hf-cache sync`: the plan against the current cache (§2.4) — files
    /// to fetch, how many were already up to date, and the fetch total.
    SyncPlanned {
        model: String,
        sha: String,
        files: Vec<FileDto>,
        skipped: usize,
        total_bytes: u64,
    },
    /// `hf-cache sync`: one staged file passed the publish gate and landed
    /// in the cache (snapshot entry + blob).
    FilePublished {
        path: String,
        blob: String,
    },
    /// `hf-cache sync`: terminal success event; in human mode the snapshot
    /// path is printed as the last output line (hf CLI parity, §2.4).
    SyncComplete {
        snapshot_path: String,
        revision: String,
        sha: String,
    },
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        available: Option<Vec<FileDto>>,
    },
}
