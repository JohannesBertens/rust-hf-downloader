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
//! - [`EngineState::enqueue`] is the single home of the enqueue transaction
//!   (registry bookkeeping → queue accounting → HUD mirror → channel sends
//!   → failed-send rollback) that four TUI flows and two CLI flows used to
//!   inline; every per-site divergence is an explicit [`EnqueuePolicy`] knob.
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

    /// The enqueue transaction (W2.1): registry bookkeeping per policy →
    /// `download_queue.add` → `download_queue_items` push + `download_tx`
    /// sends → failed-send rollback. One home for the six sequences that
    /// used to be inline (four TUI flows + two CLI flows); every divergence
    /// between them is an explicit [`EnqueuePolicy`] knob, and status/error
    /// strings stay at the call sites (see [`EnqueueOutcome`]).
    ///
    /// Lock order (AGENTS.md hierarchy): every lock is acquired in its own
    /// scope, never nested — mirror/queue/items acquisitions only ever
    /// happen sequentially, exactly like the inline code this replaces.
    ///
    /// The sender is passed in, not stored on `EngineState`: holding a
    /// sender clone would keep the channel open forever and break the CLI's
    /// drop-based deterministic completion (dropping *every* sender is the
    /// drain signal).
    pub async fn enqueue(
        &self,
        tx: &mpsc::UnboundedSender<QueuedDownload>,
        files: &[QueuedDownload],
        policy: &EnqueuePolicy,
    ) -> EnqueueOutcome {
        let total_bytes: u64 = files.iter().map(|f| f.total_size).sum();

        // --- 1. Registry bookkeeping, before any queue work (the order of
        //         every legacy site) ---------------------------------------
        let (invalid, aborted) = match &policy.registry {
            RegistryMode::None => (Vec::new(), None),
            RegistryMode::Mirror { base, entry_size } => {
                // TUI confirm flows: mirror clone → per-file validate (an
                // invalid file is skipped from the registry and reported,
                // but still queued) → append entries whose url is not
                // recorded yet → save the whole registry → re-assign the
                // mirror. Deliberately writes from the MIRROR, not from
                // disk — today's TUI behavior, kept byte-for-byte (the
                // disk-source-of-truth ops in `registry.rs` are the
                // engine/CLI flavor).
                let mut registry = self.download_registry.lock().await.clone();
                let mut skipped = Vec::new();
                for file in files {
                    let validated = match crate::paths::sanitize::validate_and_sanitize_path(
                        base,
                        &file.model_id,
                        &file.filename,
                    ) {
                        Ok(path) => path,
                        Err(e) => {
                            skipped.push((file.filename.clone(), e));
                            continue;
                        }
                    };
                    let url =
                        crate::api::resolve_url(&file.model_id, &file.filename, &file.revision);
                    if !registry.downloads.iter().any(|d| d.url == url) {
                        registry.downloads.push(DownloadMetadata {
                            model_id: file.model_id.clone(),
                            filename: file.filename.clone(),
                            url,
                            local_path: validated.to_string_lossy().to_string(),
                            total_size: match entry_size {
                                RegistryEntrySize::Zero => 0,
                                RegistryEntrySize::FromQueued => file.total_size,
                            },
                            downloaded_size: 0,
                            status: DownloadStatus::Incomplete,
                            expected_sha256: file.expected_sha256.clone(),
                            // One rule reproduces both legacy flavors: the
                            // TUI always queues the default revision (→
                            // `None`, as its inline code hardcoded) and the
                            // CLI records the revision only when it differs
                            // from the default.
                            revision: if file.revision == crate::api::DEFAULT_REVISION {
                                None
                            } else {
                                Some(file.revision.clone())
                            },
                        });
                    }
                }
                crate::registry::save_registry(&registry);
                *self.download_registry.lock().await = registry;
                (skipped, None)
            }
            RegistryMode::Disk { base } => {
                // CLI download flow: validate every file first — the first
                // invalid filename aborts the whole enqueue (nothing is
                // queued or sent) — then upsert the entries on DISK. This
                // is exactly `register_pending`, whose byte-level behavior
                // the registry golden tests pin. A single-model batch is
                // assumed (the CLI flavor's shape), so the first file's
                // model id and revision stand for the whole batch, exactly
                // like the one-`model_id`-one-`revision` call it replaces.
                let model_id = files
                    .first()
                    .map(|f| f.model_id.clone())
                    .unwrap_or_default();
                let revision = files
                    .first()
                    .map(|f| f.revision.clone())
                    .unwrap_or_else(|| crate::api::DEFAULT_REVISION.to_string());
                let pending: Vec<(String, u64, Option<String>)> = files
                    .iter()
                    .map(|f| (f.filename.clone(), f.total_size, f.expected_sha256.clone()))
                    .collect();
                (
                    Vec::new(),
                    register_pending(&model_id, &revision, &pending, base).err(),
                )
            }
        };
        if aborted.is_some() {
            // Validate-first abort: no queue work, no sends (CLI policy).
            return EnqueueOutcome {
                sent: 0,
                invalid,
                aborted,
            };
        }

        // --- 2. Queue accounting, before the sends for every flavor but
        //         resume -----------------------------------------------------
        if policy.queue == QueueTiming::BeforeSends {
            self.download_queue
                .lock()
                .await
                .add(files.len(), total_bytes);
        }

        // --- 3. HUD mirror + sends, in the per-flavor shape ----------------
        let mut sent = 0;
        match policy.items {
            ItemsMirror::AllUpfront => {
                // CLI flavors: every summary first (one lock), then sends.
                {
                    let mut items = self.download_queue_items.lock().await;
                    for file in files {
                        items.push(queue_item_summary(file));
                    }
                }
                for file in files {
                    if tx.send(file.clone()).is_ok() {
                        sent += 1;
                    }
                }
            }
            ItemsMirror::PerSuccessfulSend => {
                // TUI confirm flavors: a summary lands only for files that
                // made it onto the channel.
                for file in files {
                    if tx.send(file.clone()).is_ok() {
                        sent += 1;
                        let mut items = self.download_queue_items.lock().await;
                        items.push(queue_item_summary(file));
                    }
                }
            }
            ItemsMirror::PerSendUnconditional => {
                // Resume flavor: a summary per file regardless of send result.
                for file in files {
                    if tx.send(file.clone()).is_ok() {
                        sent += 1;
                    }
                    let mut items = self.download_queue_items.lock().await;
                    items.push(queue_item_summary(file));
                }
            }
        }

        // --- 4. Resume flavor accounts the queue once, after the sends ---
        if policy.queue == QueueTiming::AfterSends {
            self.download_queue
                .lock()
                .await
                .add(files.len(), total_bytes);
        }

        // --- 5. Failed-send rollback (TUI confirm flavors): remove the
        //         failed tail from the queue accounting — today's
        //         arithmetic, which charges the bytes of the files after
        //         the first `sent` ones (exact while failures are
        //         tail-contiguous, as they are for a closed channel). The
        //         HUD mirror needs no rollback: `PerSuccessfulSend` only
        //         ever pushed for successful sends.
        if policy.failed_send_rollback && sent < files.len() {
            let failed_bytes: u64 = files.iter().skip(sent).map(|f| f.total_size).sum();
            self.download_queue
                .lock()
                .await
                .remove(files.len() - sent, failed_bytes);
        }

        EnqueueOutcome {
            sent,
            invalid,
            aborted: None,
        }
    }
}

/// Which registry bookkeeping the enqueue transaction performs (divergence
/// knob 1 of the W2.1 table; see [`EngineState::enqueue`]).
#[derive(Debug, Clone)]
pub enum RegistryMode {
    /// Register nothing. Two deliberate flavors share this mode: the TUI
    /// resume flow re-queues entries that already exist in the registry,
    /// and `hf-cache sync` registers no pending entries at all — the named
    /// staging-sweep policy (the engine writes staging-path entries during
    /// the run; the sweeps at publish time and next bootstrap remove them,
    /// keeping the TUI's resume view clean).
    None,
    /// TUI confirm flows: read the engine's registry MIRROR, append an
    /// `Incomplete` entry for every file whose path validates and whose
    /// url is not recorded yet (invalid files are skipped and reported —
    /// they are still queued), save the whole registry to disk, then
    /// re-assign the mirror.
    Mirror {
        /// Raw user base directory used to validate each file and derive
        /// its `local_path` — NOT the queued `base_path`, which already
        /// includes `author/model` (or the staging dir).
        base: String,
        /// `total_size` recorded in the registry entries.
        entry_size: RegistryEntrySize,
    },
    /// CLI download flow: validate every file first — the first invalid
    /// filename aborts the whole enqueue (reported via
    /// [`EnqueueOutcome::aborted`]) — then upsert the entries on DISK
    /// (`register_pending`/`registry::upsert_pending`). The engine's
    /// mirror is not patched: the CLI seeds it from disk at bootstrap.
    Disk {
        /// Raw user base directory (same meaning as [`RegistryMode::Mirror`]'s
        /// `base`).
        base: String,
    },
}

/// `total_size` written into registry entries by [`RegistryMode::Mirror`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryEntrySize {
    /// The GGUF quant-group flow records `0` — today's behavior.
    Zero,
    /// Every other registering flow records the queued file size.
    FromQueued,
}

/// When `download_queue.add` runs (divergence knob 2): every flavor
/// accounts the queue BEFORE sending, except the resume flow, which sends
/// everything first and accounts once afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueTiming {
    BeforeSends,
    AfterSends,
}

/// How the `download_queue_items` HUD mirror is populated (divergence
/// knob 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemsMirror {
    /// TUI confirm flavors: after each SUCCESSFUL send, push that file's
    /// summary (failed sends never reach the HUD).
    PerSuccessfulSend,
    /// TUI resume flavor: push each file's summary right after its send,
    /// regardless of whether the send succeeded.
    PerSendUnconditional,
    /// CLI flavors: push every summary up front (one lock), before any
    /// send.
    AllUpfront,
}

/// The full shape of one enqueue transaction: every knob is a documented
/// divergence between the six legacy inline sites (see the W2.1 divergence
/// table). Use the named constructors for the six known flavors.
#[derive(Debug, Clone)]
pub struct EnqueuePolicy {
    /// Registry bookkeeping: none / TUI mirror upsert / CLI disk upsert.
    pub registry: RegistryMode,
    /// When `download_queue.add` runs relative to the sends.
    pub queue: QueueTiming,
    /// How `download_queue_items` is populated.
    pub items: ItemsMirror,
    /// Whether failed sends roll the failed tail back out of the queue
    /// accounting (divergence knob 4; only the TUI confirm flavors roll
    /// back — their HUD mirror never recorded the failed files).
    pub failed_send_rollback: bool,
}

// The two CLI constructors still have no production call site (migrated in
// the immediately following W2.1 step-3 commit); the allow dies with it.
#[allow(dead_code)]
impl EnqueuePolicy {
    /// TUI `confirm_download` (GGUF quant group): mirror registry with
    /// zero-size entries, queue accounted before the sends, HUD mirror per
    /// successful send, failed-send tail rollback.
    pub fn tui_quant(base: impl Into<String>) -> Self {
        Self {
            registry: RegistryMode::Mirror {
                base: base.into(),
                entry_size: RegistryEntrySize::Zero,
            },
            queue: QueueTiming::BeforeSends,
            items: ItemsMirror::PerSuccessfulSend,
            failed_send_rollback: true,
        }
    }

    /// TUI repository/tree confirms: mirror registry with queued-size
    /// entries; the rest exactly as [`EnqueuePolicy::tui_quant`].
    pub fn tui_repository(base: impl Into<String>) -> Self {
        Self {
            registry: RegistryMode::Mirror {
                base: base.into(),
                entry_size: RegistryEntrySize::FromQueued,
            },
            queue: QueueTiming::BeforeSends,
            items: ItemsMirror::PerSuccessfulSend,
            failed_send_rollback: true,
        }
    }

    /// TUI resume: no registry writes (entries already exist), queue
    /// accounted AFTER the sends, HUD mirror pushed unconditionally per
    /// file, no rollback.
    pub fn tui_resume() -> Self {
        Self {
            registry: RegistryMode::None,
            queue: QueueTiming::AfterSends,
            items: ItemsMirror::PerSendUnconditional,
            failed_send_rollback: false,
        }
    }

    /// CLI `download`: disk upsert via `register_pending` (validate-first,
    /// abort on the first invalid file), queue accounted before the sends,
    /// HUD mirror pushed up front, no rollback.
    pub fn cli_download(base: impl Into<String>) -> Self {
        Self {
            registry: RegistryMode::Disk { base: base.into() },
            queue: QueueTiming::BeforeSends,
            items: ItemsMirror::AllUpfront,
            failed_send_rollback: false,
        }
    }

    /// `hf-cache sync`: registers NOTHING pending — the named
    /// staging-sweep decision (purge runs at bootstrap/publish, not here;
    /// see `hf_cache_cmd.rs`); queue accounted before the sends, HUD mirror
    /// pushed up front, no rollback.
    pub fn hf_cache_sync() -> Self {
        Self {
            registry: RegistryMode::None,
            queue: QueueTiming::BeforeSends,
            items: ItemsMirror::AllUpfront,
            failed_send_rollback: false,
        }
    }
}

/// What the enqueue transaction did — the input for the call site's own
/// status/error strings (which stay at the call sites).
#[derive(Debug)]
pub struct EnqueueOutcome {
    /// Files successfully handed to the download channel.
    pub sent: usize,
    /// Files whose path validation failed during mirror registry
    /// bookkeeping, in order — skipped from the registry but STILL queued
    /// and sent. The last entry is what today's call sites left in the
    /// error field.
    pub invalid: Vec<(String, crate::paths::sanitize::PathError)>,
    /// Set only by [`RegistryMode::Disk`]'s validate-first abort: nothing
    /// was queued or sent; the caller reports the error and bails.
    // Read only by the CLI flavor + tests until the W2.1 step-3 commit
    // migrates download_cmd; the allow dies with it.
    #[allow(dead_code)]
    pub aborted: Option<crate::paths::sanitize::PathError>,
}

/// HUD summary for one queued file (the `download_queue_items` mirror).
fn queue_item_summary(file: &QueuedDownload) -> QueueItemSummary {
    QueueItemSummary {
        filename: file.filename.clone(),
        total_size: file.total_size,
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
    // Validate and build every entry first: the first invalid filename
    // aborts (via `?`) before anything is written — no partial save. The
    // registry write itself is the shared `upsert_pending` op (one load,
    // append-only-missing-urls, one save).
    let mut entries = Vec::with_capacity(files.len());
    for (filename, size, sha256) in files {
        let validated_path =
            crate::paths::sanitize::validate_and_sanitize_path(base_path, model_id, filename)?;

        let url = crate::api::resolve_url(model_id, filename, revision);
        entries.push(DownloadMetadata {
            model_id: model_id.to_string(),
            filename: filename.clone(),
            url,
            local_path: validated_path.to_string_lossy().to_string(),
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

    crate::registry::upsert_pending(&entries);
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

    // ---------------------------------------------------------------------
    // W2.1 `EngineState::enqueue` characterization tests. Each test pins one
    // row of the divergence table: the queue/accounting/items/rollback
    // behavior of one policy flavor, byte-for-byte as the inline code it
    // replaces behaved. Every test redirects HOME (registry path) and
    // HF_ENDPOINT (resolve_url) through the shared env guard, mirroring the
    // pattern above.
    // ---------------------------------------------------------------------

    /// One queued file with the given fields (revision defaults to `main`).
    fn queued_file(filename: &str, total_size: u64) -> QueuedDownload {
        QueuedDownload {
            model_id: "a/b".to_string(),
            revision: crate::api::DEFAULT_REVISION.to_string(),
            filename: filename.to_string(),
            base_path: PathBuf::from("/tmp/base/a/b"),
            expected_sha256: Some(format!("sha-{filename}")),
            hf_token: None,
            total_size,
        }
    }

    /// Drain every message currently buffered in the download channel.
    async fn drain_downloads(state: &EngineState) -> Vec<QueuedDownload> {
        let mut rx = state.download_rx.lock().await;
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    /// Queue accounting + HUD summaries + channel contents for a
    /// successful enqueue — identical for every confirm flavor.
    async fn assert_queued(state: &EngineState, files: &[QueuedDownload]) {
        let queue = state.download_queue.lock().await;
        assert_eq!(queue.size, files.len());
        assert_eq!(queue.bytes, files.iter().map(|f| f.total_size).sum::<u64>());
        drop(queue);
        let items = state.download_queue_items.lock().await;
        let want: Vec<(&str, u64)> = files
            .iter()
            .map(|f| (f.filename.as_str(), f.total_size))
            .collect();
        let got: Vec<(&str, u64)> = items
            .iter()
            .map(|i| (i.filename.as_str(), i.total_size))
            .collect();
        assert_eq!(got, want, "HUD summaries in send order");
        drop(items);
        let drained = drain_downloads(state).await;
        assert_eq!(drained.len(), files.len());
        for (got, want) in drained.iter().zip(files) {
            assert_eq!(got.filename, want.filename);
            assert_eq!(got.model_id, want.model_id);
            assert_eq!(got.revision, want.revision);
            assert_eq!(got.base_path, want.base_path);
            assert_eq!(got.expected_sha256, want.expected_sha256);
            assert_eq!(got.total_size, want.total_size);
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_tui_quant_records_zero_size_registry_entries_and_queues_all() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-enq-quant-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();
        let files = vec![queued_file("f.bin", 10), queued_file("g.bin", 20)];

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::tui_quant(tmp.to_string_lossy().to_string()),
            )
            .await;

        assert_eq!(outcome.sent, 2);
        assert!(outcome.invalid.is_empty());
        assert!(outcome.aborted.is_none());
        assert_queued(&state, &files).await;

        // GGUF quant flavor records ZERO total_size (today's quirk) and no
        // revision, with the expected sha passed through; the disk file and
        // the mirror agree (the mirror clone was saved whole).
        let mirror = state.download_registry.lock().await;
        assert_eq!(mirror.downloads.len(), 2);
        for (entry, file) in mirror.downloads.iter().zip(&files) {
            assert_eq!(entry.total_size, 0, "quant flavor records zero size");
            assert_eq!(entry.status, DownloadStatus::Incomplete);
            assert_eq!(entry.revision, None);
            assert_eq!(entry.expected_sha256, file.expected_sha256);
            assert_eq!(
                entry.url,
                crate::api::resolve_url(&file.model_id, &file.filename, &file.revision)
            );
        }
        drop(mirror);
        assert_eq!(
            crate::registry::load_registry().downloads.len(),
            2,
            "mirror clone was saved to disk"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_tui_repository_records_queued_size_registry_entries() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-enq-repo-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();
        let files = vec![queued_file("sub/f.bin", 10), queued_file("g.bin", 20)];

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::tui_repository(tmp.to_string_lossy().to_string()),
            )
            .await;

        assert_eq!(outcome.sent, 2);
        assert_queued(&state, &files).await;

        // Repository/tree flavor records the QUEUED size; local_path keeps
        // the subdirectory layout under base/author/model.
        let mirror = state.download_registry.lock().await;
        assert_eq!(mirror.downloads.len(), 2);
        assert_eq!(mirror.downloads[0].total_size, 10);
        assert_eq!(mirror.downloads[1].total_size, 20);
        let local = PathBuf::from(&mirror.downloads[0].local_path);
        assert_eq!(local, tmp.join("a").join("b").join("sub").join("f.bin"));
        drop(mirror);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_mirror_dedupes_by_url_and_skips_invalid_files_but_still_queues_them() {
        use crate::paths::sanitize::PathError;

        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-enq-dup-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();

        // The mirror already records f.bin's url (a previous confirm).
        let preexisting = DownloadMetadata {
            model_id: "a/b".to_string(),
            filename: "f.bin".to_string(),
            url: crate::api::resolve_url("a/b", "f.bin", crate::api::DEFAULT_REVISION),
            local_path: "/elsewhere/f.bin".to_string(),
            total_size: 999,
            downloaded_size: 0,
            status: DownloadStatus::Incomplete,
            expected_sha256: None,
            revision: None,
        };
        *state.download_registry.lock().await = DownloadRegistry {
            downloads: vec![preexisting.clone()],
        };

        // f.bin duplicates the recorded url; f2 duplicates f.bin WITHIN the
        // batch; ok.bin is new; ../evil.bin fails validation.
        let files = vec![
            queued_file("f.bin", 10),
            queued_file("f.bin", 10),
            queued_file("ok.bin", 20),
            queued_file("../evil.bin", 30),
        ];

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::tui_repository(tmp.to_string_lossy().to_string()),
            )
            .await;

        // Registry: only ok.bin was appended; the preexisting entry is
        // untouched (no overwrite of its local_path/size).
        let mirror = state.download_registry.lock().await;
        assert_eq!(mirror.downloads.len(), 2);
        assert_eq!(mirror.downloads[0].local_path, "/elsewhere/f.bin");
        assert_eq!(mirror.downloads[1].filename, "ok.bin");
        drop(mirror);

        // The invalid file is skipped from the registry but STILL queued and
        // sent, and reported for the call site's error string.
        assert_eq!(outcome.sent, 4);
        assert_eq!(outcome.invalid.len(), 1);
        assert_eq!(outcome.invalid[0].0, "../evil.bin");
        assert!(matches!(
            outcome.invalid[0].1,
            PathError::InvalidFilenameComponent(ref part) if part == ".."
        ));
        assert_queued(&state, &files).await;

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_disk_policy_aborts_on_first_invalid_file_without_queueing() {
        use crate::paths::sanitize::PathError;

        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-enq-abort-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();
        let files = vec![
            queued_file("f.bin", 10),
            // Windows-reserved device stem → InvalidFilenameComponent.
            queued_file("con.gguf", 20),
            queued_file("../later.bin", 30),
        ];

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::cli_download(tmp.to_string_lossy().to_string()),
            )
            .await;

        assert!(matches!(
            outcome.aborted,
            Some(PathError::InvalidFilenameComponent(_))
        ));
        assert_eq!(outcome.sent, 0);
        assert!(outcome.invalid.is_empty());

        // Abort-first: no queue work, no HUD summaries, no sends, and the
        // disk registry was never written (validate-before-write).
        let queue = state.download_queue.lock().await;
        assert_eq!((queue.size, queue.bytes), (0, 0));
        drop(queue);
        assert!(state.download_queue_items.lock().await.is_empty());
        assert!(drain_downloads(&state).await.is_empty());
        assert!(crate::registry::load_registry().downloads.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_disk_policy_upserts_disk_registry_not_the_mirror() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-enq-disk-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        // Disk already records f.bin's url → the upsert must not duplicate it.
        crate::registry::save_registry(&DownloadRegistry {
            downloads: vec![DownloadMetadata {
                model_id: "a/b".to_string(),
                filename: "f.bin".to_string(),
                url: crate::api::resolve_url("a/b", "f.bin", "deadbeef"),
                local_path: "/elsewhere/f.bin".to_string(),
                total_size: 999,
                downloaded_size: 0,
                status: DownloadStatus::Incomplete,
                expected_sha256: None,
                revision: None,
            }],
        });

        let (state, tx) = EngineState::new();
        let mut files = vec![queued_file("f.bin", 10), queued_file("g.bin", 20)];
        // Non-default revision, like `--revision deadbeef`.
        for file in &mut files {
            file.revision = "deadbeef".to_string();
        }

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::cli_download(tmp.to_string_lossy().to_string()),
            )
            .await;

        assert_eq!(outcome.sent, 2);
        assert_queued(&state, &files).await;

        // Disk: only the missing url was appended; the new entry records
        // the non-default revision (and the url was resolved against it).
        let disk = crate::registry::load_registry();
        assert_eq!(disk.downloads.len(), 2);
        assert_eq!(disk.downloads[1].filename, "g.bin");
        assert_eq!(disk.downloads[1].revision.as_deref(), Some("deadbeef"));
        assert_eq!(disk.downloads[1].total_size, 20);
        assert_eq!(
            disk.downloads[1].url,
            crate::api::resolve_url("a/b", "g.bin", "deadbeef")
        );

        // Mirror: deliberately NOT patched — the CLI seeds it from disk at
        // bootstrap, so an unseeded engine keeps its (empty) mirror.
        assert!(state.download_registry.lock().await.downloads.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Close the download channel from the receiver side so every send
    /// fails — the only way `send` on an unbounded channel can fail.
    async fn close_download_channel(state: &EngineState) {
        state.download_rx.lock().await.close();
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_failed_sends_roll_back_queue_accounting_for_tui_confirm_flavors() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-enq-roll-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();
        close_download_channel(&state).await;
        let files = vec![queued_file("f.bin", 10), queued_file("g.bin", 20)];

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::tui_repository(tmp.to_string_lossy().to_string()),
            )
            .await;

        // All sends failed → the pre-send queue.add(num, total) is rolled
        // back to zero (remove(num - sent, tail bytes), sent = 0), the HUD
        // mirror never recorded anything — identical to today's inline
        // rollback — while the registry bookkeeping STAYS (it ran first and
        // was never rolled back).
        assert_eq!(outcome.sent, 0);
        let queue = state.download_queue.lock().await;
        assert_eq!((queue.size, queue.bytes), (0, 0));
        drop(queue);
        assert!(state.download_queue_items.lock().await.is_empty());
        assert_eq!(state.download_registry.lock().await.downloads.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_resume_flavor_accounts_after_sends_and_never_rolls_back() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-enq-resume-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();
        close_download_channel(&state).await;
        let files = vec![queued_file("f.bin", 10), queued_file("g.bin", 20)];

        let outcome = state
            .enqueue(&tx, &files, &EnqueuePolicy::tui_resume())
            .await;

        // Resume flavor divergence, pinned: every send failed, yet the HUD
        // summaries were still pushed (one per file), the queue was
        // accounted AFTER the loop and never rolled back, and no registry
        // write happened at all (entries already exist on disk).
        assert_eq!(outcome.sent, 0);
        let queue = state.download_queue.lock().await;
        assert_eq!(queue.size, 2);
        assert_eq!(queue.bytes, 30);
        drop(queue);
        assert_eq!(state.download_queue_items.lock().await.len(), 2);
        assert!(state.download_registry.lock().await.downloads.is_empty());
        assert!(crate::registry::load_registry().downloads.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_cli_flavors_push_items_upfront_without_rollback() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-enq-cli-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        // Both CLI flavors with a dead channel: HUD summaries land up front
        // (before any send), the queue accounting stays charged (no
        // rollback), and — for hf-cache — no registry write happens.
        let policies = vec![
            EnqueuePolicy::cli_download(tmp.to_string_lossy().to_string()),
            EnqueuePolicy::hf_cache_sync(),
        ];
        for policy in &policies {
            let (state, tx) = EngineState::new();
            close_download_channel(&state).await;
            let files = vec![queued_file("f.bin", 10), queued_file("g.bin", 20)];

            let outcome = state.enqueue(&tx, &files, policy).await;

            assert_eq!(outcome.sent, 0, "channel closed");
            assert!(outcome.aborted.is_none(), "valid files");
            let queue = state.download_queue.lock().await;
            assert_eq!(queue.size, 2, "CLI flavors never roll back");
            assert_eq!(queue.bytes, 30);
            drop(queue);
            assert_eq!(state.download_queue_items.lock().await.len(), 2);
            assert!(
                state.download_registry.lock().await.downloads.is_empty(),
                "hf-cache registers nothing; cli_download writes disk, not the mirror"
            );
        }

        // The cli_download flavor DID write its pending entries to disk
        // (hf-cache's pass leaves the registry file untouched).
        assert_eq!(crate::registry::load_registry().downloads.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
