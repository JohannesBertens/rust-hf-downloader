//! Progress, queue, registry, and verification types exchanged with the
//! engine (download manager and verification worker).

use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

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
}
