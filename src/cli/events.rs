//! Stable, additive-only NDJSON event schema (snapshot-tested).

use serde::{Deserialize, Serialize};

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

/// Stable error codes carried by [`Event::Error`] (wire contract, plan
/// H6: exactly the 13 codes pinned by `error_event_code_wire_contract_table`).
/// The enum is deliberately NOT serde-derived — `Event::Error` keeps a
/// `code: String` field and constructors serialize via [`ErrorCode::as_str`],
/// which is what keeps the NDJSON bytes identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Usage,
    InvalidPath,
    Interrupted,
    DownloadFailed,
    HashMismatch,
    AuthRequired,
    VerificationError,
    PlanFailed,
    Io,
    PublishFailed,
    SyncLock,
    Internal,
    Network,
}

impl ErrorCode {
    /// The wire string for this code (no allocation).
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::Usage => "usage",
            ErrorCode::InvalidPath => "invalid_path",
            ErrorCode::Interrupted => "interrupted",
            ErrorCode::DownloadFailed => "download_failed",
            ErrorCode::HashMismatch => "hash_mismatch",
            ErrorCode::AuthRequired => "auth_required",
            ErrorCode::VerificationError => "verification_error",
            ErrorCode::PlanFailed => "plan_failed",
            ErrorCode::Io => "io",
            ErrorCode::PublishFailed => "publish_failed",
            ErrorCode::SyncLock => "sync_lock",
            ErrorCode::Internal => "internal",
            ErrorCode::Network => "network",
        }
    }
}

/// Terminal status of one downloaded file (wire contract, plan H6: both
/// literals pinned by `file_complete_status_wire_contract`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Downloaded,
    AlreadyExists,
}

impl Event {
    /// Error event without an `available` list. Use this (with an
    /// [`ErrorCode`]) instead of raw struct construction so the code string
    /// stays tied to the pinned wire set.
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Event {
        Event::Error {
            code: code.as_str().to_string(),
            message: message.into(),
            available: None,
        }
    }

    /// Error event with an `available` list of files. For codes outside the
    /// H6 set (resolve/selection errors like `ambiguous`), construct
    /// [`Event::Error`] directly — that is why the field stays `String`.
    ///
    /// No current H6-code construction site carries an `available` list (the
    /// two available-bearing sites emit resolve/selection codes outside the
    /// pinned 13), so this has no production caller yet; its wire shape is
    /// pinned by `error_with_available_constructor_wire_shape`.
    #[allow(dead_code)]
    pub fn error_with_available(
        code: ErrorCode,
        message: impl Into<String>,
        available: Vec<FileDto>,
    ) -> Event {
        Event::Error {
            code: code.as_str().to_string(),
            message: message.into(),
            available: Some(available),
        }
    }
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
        status: FileStatus,
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
    /// `hf-cache sync`: the plan against the current cache (plans/hf-cache-sync.md §2.4) — files
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
    /// path is printed as the last output line (hf CLI parity, plans/hf-cache-sync.md §2.4).
    SyncComplete {
        snapshot_path: String,
        revision: String,
        sha: String,
    },
    /// A non-fatal warning the run surfaced (e.g. the dropped-malformed-
    /// HF-token warning from bootstrap: requests proceed unauthenticated).
    /// Additive (2026-10-07, B5 owner revision); human mode renders it as
    /// a `Warning: …` stderr line.
    Warning {
        message: String,
    },
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        available: Option<Vec<FileDto>>,
    },
}
