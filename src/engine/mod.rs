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
//!   inline; every per-site divergence is an explicit, constructor-sealed
//!   [`EnqueuePolicy`] knob.
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
//!
//! # Layout
//!
//! This file is a facade over three private submodules (the `models/`
//! precedent — every `crate::engine::X` import keeps compiling unchanged):
//!
//! - `enqueue` — [`EngineState::enqueue`] plus the sealed [`EnqueuePolicy`]
//!   knob types (`RegistryMode`, `SendDiscipline`, `InvalidPolicy`,
//!   [`EnqueueOutcome`]) and their characterization tests.
//! - `workers` — [`spawn_manager`], [`spawn_verification_worker`], and the
//!   [`ManagerHandle`] drain contract.
//! - `bootstrap` — [`bootstrap`] (CLI startup sequence) and
//!   [`seed_registry_mirror`].
//!
//! [`EngineState`] itself (with the [`QueuedDownload`] message type it
//! routes) lives here in the facade.

mod bootstrap;
mod enqueue;
mod workers;

pub use bootstrap::{bootstrap, seed_registry_mirror};
pub use enqueue::{EnqueueOutcome, EnqueuePolicy};
pub use workers::{spawn_manager, spawn_verification_worker, ManagerHandle};

use crate::models::{
    CompleteDownloads, DownloadProgress, DownloadRegistry, FileOutcome, QueueItemSummary,
    QueueState, VerificationProgress, VerificationQueueItem, VerifyOutcome,
};
use crate::verification::VerificationResultCounters;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

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

/// Auth-status channel contract (W2.6). When a download hits HTTP 401, the
/// engine sends `AUTH_ERROR:<model_id>` on the free-text status channel;
/// both frontends detect that line through [`parse_auth_status`] (the TUI
/// opens the AuthError popup from it, the human CLI prints its auth hint).
/// The typed signal lives in the outcome stream
/// (`FileOutcome::AuthRequired`), which the CLI maps to
/// `error [auth_required]` / `EXIT_AUTH`. Defining the string in exactly
/// one place (builder + parser here, one producer in `download.rs`, two
/// consumers calling this parser) removes the duplicated string contract
/// without changing a byte on the wire; a future typed-event migration
/// swaps this one function instead of N call sites.
pub const AUTH_STATUS_PREFIX: &str = "AUTH_ERROR:";

/// Build the auth-status line for `model_id` — byte-identical to the
/// previous inline `format!("AUTH_ERROR:{}", model_id)` producer.
pub fn auth_status_message(model_id: &str) -> String {
    format!("{AUTH_STATUS_PREFIX}{model_id}")
}

/// Parse a status line back into its auth model id; `None` for any other
/// status line. `Some("")` for the bare prefix pins the
/// `format!`/`strip_prefix` round-trip behavior.
pub fn parse_auth_status(status: &str) -> Option<&str> {
    status.strip_prefix(AUTH_STATUS_PREFIX)
}

/// Env plumbing shared by the engine submodule tests (`enqueue`, `workers`,
/// `bootstrap`): the EnvGuard redirects `RUST_HF_DOWNLOADER_DATA_DIR`
/// (registry path — `HOME` alone does not isolate on Windows, where
/// `dirs::home_dir()` reads `USERPROFILE` instead) and `HF_ENDPOINT`
/// (resolve_url) and restores both on drop.
#[cfg(test)]
pub(crate) mod test_support {
    /// Find a guaranteed-closed localhost port (bind then drop the listener).
    pub(crate) fn closed_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    pub(crate) struct EnvGuard {
        data_dir: Option<std::ffi::OsString>,
        endpoint: Option<String>,
    }

    impl EnvGuard {
        pub(crate) fn install(data_dir: &std::path::Path, endpoint: &str) -> Self {
            let guard = Self {
                data_dir: std::env::var_os(crate::paths::ENV_DATA_DIR),
                endpoint: std::env::var("HF_ENDPOINT").ok(),
            };
            std::env::set_var(crate::paths::ENV_DATA_DIR, data_dir);
            std::env::set_var("HF_ENDPOINT", endpoint);
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.data_dir {
                Some(d) => std::env::set_var(crate::paths::ENV_DATA_DIR, d),
                None => std::env::remove_var(crate::paths::ENV_DATA_DIR),
            }
            match &self.endpoint {
                Some(e) => std::env::set_var("HF_ENDPOINT", e),
                None => std::env::remove_var("HF_ENDPOINT"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// W2.6: the auth-status line's exact bytes and both parse outcomes are
    /// the pinned contract shared by the download-task producer and the TUI
    /// and human-CLI consumers (the typed signal is `FileOutcome::AuthRequired`).
    #[test]
    fn auth_status_string_contract_is_pinned() {
        // Producer: byte-identical to the legacy inline format!.
        assert_eq!(
            auth_status_message("meta-llama/Llama-3-8B-Instruct"),
            "AUTH_ERROR:meta-llama/Llama-3-8B-Instruct"
        );
        // Consumers: the same model id both frontends extracted via
        // strip_prefix before the parser was centralized.
        assert_eq!(
            parse_auth_status("AUTH_ERROR:meta-llama/Llama-3-8B-Instruct"),
            Some("meta-llama/Llama-3-8B-Instruct")
        );
        // Non-auth status lines must not match (they reach the status
        // handlers verbatim).
        assert_eq!(
            parse_auth_status("Error: Download failed after retries: boom"),
            None
        );
        assert_eq!(parse_auth_status(""), None);
        // Bare prefix pins the format!/strip_prefix round-trip (empty id).
        assert_eq!(parse_auth_status(AUTH_STATUS_PREFIX), Some(""));
        // Builder/parser round-trip.
        assert_eq!(parse_auth_status(&auth_status_message("a/b")), Some("a/b"));
    }
}
