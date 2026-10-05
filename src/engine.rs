//! Shared download engine used by both the TUI and the CLI frontends.
//!
//! Historically the download-manager loop (and the verification-worker
//! bootstrap) was spawned inline in `App::run`, and the removed v1 headless
//! CLI duplicated that bootstrap in `main.rs` — the two copies drifted until
//! the CLI was deleted in v2.0.0. This module is the single, shared
//! implementation both frontends consume:
//!
//! - [`spawn_manager`] consumes the `download_tx` channel (the "queue") and
//!   drives [`crate::download::start_download`] one file at a time, exactly
//!   like the TUI always did.
//! - [`spawn_verification_worker`] runs the background SHA256 worker.
//! - [`bootstrap`] is the full startup sequence the CLI frontends use
//!   (fresh state → registry-mirror seed → both spawns); [`seed_registry_mirror`]
//!   is the shared mirror-seed step inside it, also reused by the TUI's
//!   startup scan.
//! - Dropping *every* clone of `download_tx` closes the channel; the manager
//!   then drains its queue and the returned join handle resolves with one
//!   [`FileOutcome`] per processed file. This is how the CLI gets
//!   deterministic completion without polling heuristics.
//! - [`EngineState::verification_idle`] is a race-free "no verification work
//!   pending or running" signal (the in-flight counter is incremented while
//!   the queue lock is held, before an item is removed).

use crate::download::{start_download, DownloadParams};
use crate::models::{
    CompleteDownloads, DownloadMetadata, DownloadProgress, DownloadRegistry, DownloadStatus,
    FileOutcome, QueueItemSummary, QueueState, VerificationProgress, VerificationQueueItem,
    VerifyOutcome,
};
use crate::verification::VerificationResultCounters;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

/// One queued download, sent over the `download_tx` channel to the manager.
/// `revision` is a branch, tag, or commit SHA (issue #28); it flows into
/// resolve URLs and registry entries.
#[derive(Debug, Clone)]
pub struct QueuedDownload {
    pub model_id: String,
    pub revision: String,
    pub filename: String,
    /// Model root directory (base/author/model); the filename's subpath is
    /// appended during download.
    pub base_path: PathBuf,
    /// Expected SHA256 (LFS oid) when known; `None` skips hash checking.
    pub expected_sha256: Option<String>,
    /// Hugging Face auth token, if the session has one.
    pub hf_token: Option<String>,
    /// Total file size in bytes, for queue accounting and progress.
    pub total_size: u64,
}

/// Type alias for download receiver to reduce complexity
pub type DownloadReceiver = Arc<Mutex<mpsc::UnboundedReceiver<QueuedDownload>>>;

/// The bundle of shared handles the engine tasks and frontends communicate
/// through. Every field is an Arc or a channel endpoint, so cloning is cheap.
#[derive(Clone, Debug)]
pub struct EngineState {
    pub download_rx: DownloadReceiver,
    pub download_queue: Arc<Mutex<QueueState>>,
    pub download_queue_items: Arc<Mutex<Vec<QueueItemSummary>>>,
    pub download_progress: Arc<Mutex<Option<DownloadProgress>>>,
    pub complete_downloads: Arc<Mutex<CompleteDownloads>>,
    pub status_tx: mpsc::UnboundedSender<String>,
    pub status_rx: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    pub verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    pub verification_queue_size: Arc<AtomicUsize>,
    /// Number of verification tasks spawned but not yet finished. Incremented
    /// while the queue lock is held (before the item is removed), so
    /// `queue_size == 0 && in_flight == 0` can never observe a false idle
    /// between the queue removal and the task start.
    pub verification_in_flight: Arc<AtomicUsize>,
    pub verification_progress: Arc<Mutex<Vec<VerificationProgress>>>,
    pub download_registry: Arc<Mutex<DownloadRegistry>>,
    /// Typed verification results. The TUI ignores this channel (it renders
    /// from `verification_progress` and status strings); the CLI consumes it
    /// for JSON events and exit codes.
    pub verify_tx: mpsc::UnboundedSender<VerifyOutcome>,
    pub verify_rx: Arc<Mutex<mpsc::UnboundedReceiver<VerifyOutcome>>>,
    /// Per-file download outcomes, streamed by the manager as each file
    /// finishes (the join handle additionally returns the full list). The TUI
    /// ignores this channel; the CLI consumes it for live events.
    pub outcome_tx: mpsc::UnboundedSender<FileOutcome>,
    pub outcome_rx: Arc<Mutex<mpsc::UnboundedReceiver<FileOutcome>>>,
    /// Session-lifetime verification counters (HUD footer / CLI summary)
    pub verification_results: VerificationResultCounters,
}

impl EngineState {
    /// Create a fresh, fully connected engine state. Returns the state bundle
    /// plus the sender half of the download channel. Dropping *all* clones of
    /// the sender ends the manager loop once the queue drains.
    pub fn new() -> (Self, mpsc::UnboundedSender<QueuedDownload>) {
        let (download_tx, download_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        let (verify_tx, verify_rx) = mpsc::unbounded_channel();
        let (outcome_tx, outcome_rx) = mpsc::unbounded_channel();

        (
            Self {
                download_rx: Arc::new(Mutex::new(download_rx)),
                download_queue: Arc::new(Mutex::new(QueueState::new(0, 0))),
                download_queue_items: Arc::new(Mutex::new(Vec::new())),
                download_progress: Arc::new(Mutex::new(None)),
                complete_downloads: Arc::new(Mutex::new(std::collections::HashMap::new())),
                status_tx,
                status_rx: Arc::new(Mutex::new(status_rx)),
                verification_queue: Arc::new(Mutex::new(Vec::new())),
                verification_queue_size: Arc::new(AtomicUsize::new(0)),
                verification_in_flight: Arc::new(AtomicUsize::new(0)),
                verification_progress: Arc::new(Mutex::new(Vec::new())),
                download_registry: Arc::new(Mutex::new(DownloadRegistry::default())),
                verify_tx,
                verify_rx: Arc::new(Mutex::new(verify_rx)),
                outcome_tx,
                outcome_rx: Arc::new(Mutex::new(outcome_rx)),
                verification_results: VerificationResultCounters::default(),
            },
            download_tx,
        )
    }

    /// True when no verification work is queued or running.
    ///
    /// Only meaningful after all downloads have drained: every
    /// `queue_verification` call happens inside `start_download`, which the
    /// manager awaits, so once the manager join handle has resolved, all
    /// queue pushes have happened and this condition is stable.
    pub fn verification_idle(&self) -> bool {
        self.verification_queue_size.load(Ordering::Relaxed) == 0
            && self.verification_in_flight.load(Ordering::Relaxed) == 0
    }
}

/// Handle to the running download manager.
pub struct ManagerHandle {
    /// Resolves with one [`FileOutcome`] per processed file once the download
    /// channel is closed and fully drained. The TUI simply drops this handle
    /// (the task keeps running); the CLI awaits it for completion.
    pub join: JoinHandle<Vec<FileOutcome>>,
}

/// Spawn the download manager task.
///
/// Consumes the `download_rx` channel serially (one file at a time;
/// parallelism is within a file's chunks, provided by
/// [`crate::download::start_download`]) and maintains the queue accounting
/// the TUI HUD renders from.
pub fn spawn_manager(state: EngineState) -> ManagerHandle {
    let join = tokio::spawn(async move {
        let mut outcomes = Vec::new();

        loop {
            // Lock only when receiving, release immediately after. This
            // prevents deadlock by not holding download_rx while acquiring
            // other locks (see AGENTS.md lock hierarchy).
            let download = {
                let mut rx = state.download_rx.lock().await;
                match rx.recv().await {
                    Some(msg) => msg,
                    None => break, // Channel closed and drained
                }
            };
            let QueuedDownload {
                model_id,
                revision,
                filename,
                base_path,
                expected_sha256,
                hf_token,
                total_size,
            } = download;

            // Decrement queue size and bytes when we start processing
            {
                let mut queue = state.download_queue.lock().await;
                queue.remove(1, total_size);
            }
            // Remove the mirrored queue item (first match by filename)
            {
                let mut items = state.download_queue_items.lock().await;
                if let Some(pos) = items.iter().position(|it| it.filename == filename) {
                    items.remove(pos);
                }
            }

            let outcome = start_download(DownloadParams {
                model_id,
                revision,
                filename,
                base_path,
                progress: state.download_progress.clone(),
                status_tx: state.status_tx.clone(),
                complete_downloads: state.complete_downloads.clone(),
                expected_sha256,
                verification_queue: state.verification_queue.clone(),
                verification_queue_size: state.verification_queue_size.clone(),
                hf_token,
            })
            .await;

            // Stream per-file outcomes to live consumers (the join handle
            // still returns the complete list for drain-based callers).
            let _ = state.outcome_tx.send(outcome.clone());

            outcomes.push(outcome);
        }

        outcomes
    });

    ManagerHandle { join }
}

/// Spawn the background verification worker (runs until the process exits).
pub fn spawn_verification_worker(state: EngineState) -> JoinHandle<()> {
    tokio::spawn(crate::verification::verification_worker(state))
}

/// Load the on-disk registry into the engine's `download_registry` mirror
/// and return the loaded snapshot.
///
/// This is the single home of the startup mirror-seed step both frontends
/// used to inline (the CLI bootstrap sites and the TUI's startup scan):
/// without it, registry updates written by the engine cannot find their
/// entries. The snapshot is returned so callers that also derive views from
/// it (the TUI's incomplete/complete lists) read the registry from disk
/// exactly once and see one consistent view.
pub async fn seed_registry_mirror(state: &EngineState) -> DownloadRegistry {
    let registry = crate::registry::load_registry();
    *state.download_registry.lock().await = registry.clone();
    registry
}

/// Bootstrap the shared engine the way the CLI frontends do:
/// [`EngineState::new`] → [`seed_registry_mirror`] →
/// [`spawn_verification_worker`] → [`spawn_manager`], in exactly that order.
///
/// `download_tx` is the sender half of the download channel: one-shot
/// callers (the CLI) send their queue and then drop it — closing the channel
/// is the deterministic completion signal (the manager's join handle
/// resolves). The verification worker's join handle is detached, exactly
/// like the inline bootstrap this replaces.
///
/// The TUI cannot use this function — its `App::new` is sync and must build
/// the engine before any await point — so it composes the same pieces:
/// [`EngineState::new`] in `App::new`, [`seed_registry_mirror`] in its
/// startup scan, the two spawns in `App::run`.
pub async fn bootstrap() -> (
    EngineState,
    mpsc::UnboundedSender<QueuedDownload>,
    ManagerHandle,
) {
    let (state, download_tx) = EngineState::new();
    seed_registry_mirror(&state).await;
    spawn_verification_worker(state.clone());
    let manager = spawn_manager(state.clone());
    (state, download_tx, manager)
}

/// Seed the on-disk registry with `Incomplete` entries for files about to be
/// queued, so downloads started headlessly show up in the TUI's
/// resume/complete views. Validates each filename (path-traversal safety,
/// same rules as the TUI) and returns the first validation error, if any.
/// The error type is the shared [`crate::paths::sanitize::PathError`]:
/// path validation is register_pending's only failure source today, and
/// its `Display` reproduces the historical message strings byte-for-byte.
pub fn register_pending(
    model_id: &str,
    revision: &str,
    files: &[(String, u64, Option<String>)],
    base_path: &str,
) -> Result<(), crate::paths::sanitize::PathError> {
    let mut registry = crate::registry::load_registry();

    for (filename, size, sha256) in files {
        let validated_path =
            crate::paths::sanitize::validate_and_sanitize_path(base_path, model_id, filename)?;

        let url = crate::api::resolve_url(model_id, filename, revision);
        let local_path_str = validated_path.to_string_lossy().to_string();

        if !registry.downloads.iter().any(|d| d.url == url) {
            registry.downloads.push(DownloadMetadata {
                model_id: model_id.to_string(),
                filename: filename.clone(),
                url,
                local_path: local_path_str,
                total_size: *size,
                downloaded_size: 0,
                status: DownloadStatus::Incomplete,
                expected_sha256: sha256.clone(),
                revision: if revision == crate::api::DEFAULT_REVISION {
                    None
                } else {
                    Some(revision.to_string())
                },
            });
        }
    }

    crate::registry::save_registry(&registry);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env-mutating tests share the crate-wide mutex from `paths`: these
    // tests redirect `HOME` (which moves the registry path), the registry
    // golden tests in `registry_tests` redirect `ENV_DATA_DIR` (which moves
    // it too) — one mutex serializes every test that touches the
    // process-global registry location. (`cargo test` runs unit tests in
    // parallel threads within one process.)
    use crate::paths::ENV_MUTEX;

    /// Find a guaranteed-closed localhost port (bind then drop the listener).
    fn closed_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    struct EnvGuard {
        home: Option<String>,
        endpoint: Option<String>,
    }

    impl EnvGuard {
        fn install(home: &std::path::Path, endpoint: &str) -> Self {
            let guard = Self {
                home: std::env::var("HOME").ok(),
                endpoint: std::env::var("HF_ENDPOINT").ok(),
            };
            std::env::set_var("HOME", home);
            std::env::set_var("HF_ENDPOINT", endpoint);
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            match &self.endpoint {
                Some(e) => std::env::set_var("HF_ENDPOINT", e),
                None => std::env::remove_var("HF_ENDPOINT"),
            }
        }
    }

    #[tokio::test]
    // Holding the (std) env mutex across the await below is intentional: it
    // serializes env-mutating tests; other tokio workers keep making progress.
    #[allow(clippy::await_holding_lock)]
    async fn manager_drains_when_channel_closed() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-drain-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        // Fail fast: no retries
        let old_retries = crate::download::DOWNLOAD_CONFIG
            .max_retries
            .load(Ordering::Relaxed);
        crate::download::DOWNLOAD_CONFIG
            .max_retries
            .store(0, Ordering::Relaxed);

        let (state, tx) = EngineState::new();
        let handle = spawn_manager(state.clone());

        for name in ["f.bin", "g.bin"] {
            tx.send(QueuedDownload {
                model_id: "a/b".to_string(),
                revision: crate::api::DEFAULT_REVISION.to_string(),
                filename: name.to_string(),
                base_path: tmp.clone(),
                expected_sha256: None,
                hf_token: None,
                total_size: 10,
            })
            .unwrap();
        }
        drop(tx); // closes the channel → manager drains and resolves

        let outcomes = handle.join.await.expect("manager task panicked");
        assert_eq!(outcomes.len(), 2, "one outcome per queued file");
        assert!(
            matches!(&outcomes[0], FileOutcome::Failed { filename, .. } if filename == "f.bin")
        );
        assert!(
            matches!(&outcomes[1], FileOutcome::Failed { filename, .. } if filename == "g.bin")
        );

        // Queue accounting drained back to zero
        assert_eq!(state.download_queue.lock().await.size, 0);

        crate::download::DOWNLOAD_CONFIG
            .max_retries
            .store(old_retries, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn queued_download_fields_roundtrip_through_the_channel() {
        // QueuedDownload is a named struct, so the two adjacent
        // Option<String> fields (expected_sha256 / hf_token) can no longer
        // be swapped at a construction site without the compiler catching
        // it. Pin the field meaning by roundtripping one message through the
        // engine channel and reading every field back by name.
        let (state, tx) = EngineState::new();
        tx.send(QueuedDownload {
            model_id: "author/model".to_string(),
            revision: "deadbeef".to_string(),
            filename: "sub/dir/file.bin".to_string(),
            base_path: PathBuf::from("/tmp/base/author/model"),
            expected_sha256: Some("abc123".to_string()),
            hf_token: None,
            total_size: 42,
        })
        .unwrap();
        drop(tx);

        let received = { state.download_rx.lock().await.recv().await }.expect("message queued");
        assert_eq!(received.model_id, "author/model");
        assert_eq!(received.revision, "deadbeef");
        assert_eq!(received.filename, "sub/dir/file.bin");
        assert_eq!(received.base_path, PathBuf::from("/tmp/base/author/model"));
        assert_eq!(received.expected_sha256.as_deref(), Some("abc123"));
        assert_eq!(received.hf_token, None);
        assert_eq!(received.total_size, 42);
    }

    #[tokio::test]
    async fn verification_idle_tracks_queue_and_in_flight() {
        let (state, _tx) = EngineState::new();

        assert!(state.verification_idle(), "fresh engine is idle");

        state
            .verification_queue_size
            .fetch_add(1, Ordering::Relaxed);
        assert!(!state.verification_idle(), "queued work is not idle");
        state
            .verification_queue_size
            .fetch_sub(1, Ordering::Relaxed);

        state.verification_in_flight.fetch_add(1, Ordering::Relaxed);
        assert!(!state.verification_idle(), "in-flight work is not idle");
        state.verification_in_flight.fetch_sub(1, Ordering::Relaxed);

        assert!(state.verification_idle(), "idle again after drain");
    }

    /// One registry entry to write to disk, exercising the same field set
    /// `register_pending` produces.
    fn sample_registry() -> DownloadRegistry {
        DownloadRegistry {
            downloads: vec![DownloadMetadata {
                model_id: "a/b".to_string(),
                filename: "f.bin".to_string(),
                url: "https://example.invalid/a/b/resolve/main/f.bin".to_string(),
                local_path: "/tmp/f.bin".to_string(),
                total_size: 10,
                downloaded_size: 0,
                status: DownloadStatus::Incomplete,
                expected_sha256: None,
                revision: None,
            }],
        }
    }

    #[tokio::test]
    // Holding the (std) env mutex across the awaits below is intentional:
    // it serializes env-mutating tests; other tokio workers keep making
    // progress.
    #[allow(clippy::await_holding_lock)]
    async fn seed_registry_mirror_loads_disk_into_mirror_and_returns_snapshot() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-seed-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, _tx) = EngineState::new();

        // No registry file on disk → empty mirror, empty snapshot (the
        // pre-seed mirror state is also empty, so this pins the default).
        let snapshot = seed_registry_mirror(&state).await;
        assert!(snapshot.downloads.is_empty());
        assert!(state.download_registry.lock().await.downloads.is_empty());

        // Registry file with one entry → both the mirror and the returned
        // snapshot match what is on disk (whole-registry assign, the
        // semantics both CLI bootstrap sites and the TUI scan used inline).
        crate::registry::save_registry(&sample_registry());
        let snapshot = seed_registry_mirror(&state).await;
        assert_eq!(snapshot.downloads.len(), 1);
        assert_eq!(snapshot.downloads[0].filename, "f.bin");
        let mirror = state.download_registry.lock().await;
        assert_eq!(mirror.downloads.len(), 1);
        assert_eq!(mirror.downloads[0].filename, snapshot.downloads[0].filename);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn bootstrap_seeds_mirror_and_spawns_draining_manager() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-bootstrap-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));
        crate::registry::save_registry(&sample_registry());

        let (state, download_tx, manager) = bootstrap().await;

        // Mirror seeded from disk as part of bootstrap, before the workers
        // were handed the state (the seeded mirror is what verification
        // updates look their entries up in).
        assert_eq!(state.download_registry.lock().await.downloads.len(), 1);

        // The manager was spawned: closing the channel makes its join
        // handle resolve (zero files queued → empty outcome list).
        drop(download_tx);
        let outcomes = manager.join.await.expect("manager task panicked");
        assert!(outcomes.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
