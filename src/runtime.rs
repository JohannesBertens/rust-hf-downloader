//! Shared download/verification runtime.
//!
//! [`DownloadRuntime`] owns the cross-task channels and shared-state handles
//! that the verification worker, the download manager, and the UI/CLI
//! front-ends communicate through. It is constructed once (identically) by
//! both the TUI (`ui::App`) and the headless CLI (`main.rs`) paths, which
//! removes the previous duplication of channel/Arc wiring in two places.
//!
//! # Scheduling divergence (intentional)
//!
//! The two front-ends schedule downloads differently, so two manager
//! variants are provided:
//!
//! - [`DownloadRuntime::spawn_download_manager_serial`] (TUI): awaits each
//!   download inline and accounts queue size when a download *starts*. The
//!   TUI tracks a single active download's progress, so it runs serially.
//! - [`DownloadRuntime::spawn_download_manager_concurrent`] (headless):
//!   spawns a task per download (concurrent) and accounts queue size on
//!   *completion*.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use crate::download::{start_download, DownloadConfig, DownloadParams};
use crate::models::{
    CompleteDownloads, DownloadMessage, DownloadProgress, DownloadRegistry, QueueState,
    VerificationProgress, VerificationQueueItem,
};
use crate::rate_limiter::RateLimiter;
use crate::verification::VerificationConfig;

/// Shared runtime state for the download + verification engine.
///
/// Built once via [`DownloadRuntime::new`] and reused by both front-ends.
/// Field types follow the project's documented lock hierarchy.
pub struct DownloadRuntime {
    pub download_tx: mpsc::UnboundedSender<DownloadMessage>,
    pub download_rx: Arc<Mutex<mpsc::UnboundedReceiver<DownloadMessage>>>,
    pub status_tx: mpsc::UnboundedSender<String>,
    pub status_rx: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    pub auth_tx: mpsc::UnboundedSender<String>,
    pub auth_rx: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    pub download_progress: Arc<Mutex<Option<DownloadProgress>>>,
    pub complete_downloads: Arc<Mutex<CompleteDownloads>>,
    pub download_queue: Arc<Mutex<QueueState>>,
    pub verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    pub verification_queue_size: Arc<AtomicUsize>,
    pub verification_progress: Arc<Mutex<Vec<VerificationProgress>>>,
    pub download_registry: Arc<Mutex<DownloadRegistry>>,
    pub download_config: Arc<DownloadConfig>,
    pub rate_limiter: RateLimiter,
    pub verification_config: Arc<VerificationConfig>,
}

impl DownloadRuntime {
    /// Construct all channels and shared-state handles with fresh defaults.
    pub fn new() -> Self {
        let (download_tx, download_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        let (auth_tx, auth_rx) = mpsc::unbounded_channel();
        let download_config = Arc::new(DownloadConfig::new());
        let rate_limiter = RateLimiter::new(
            download_config.rate_limit_bytes_per_sec.load(Ordering::Relaxed),
            2.0,
        );
        Self {
            download_tx,
            download_rx: Arc::new(Mutex::new(download_rx)),
            status_tx,
            status_rx: Arc::new(Mutex::new(status_rx)),
            auth_tx,
            auth_rx: Arc::new(Mutex::new(auth_rx)),
            download_progress: Arc::new(Mutex::new(None)),
            complete_downloads: Arc::new(Mutex::new(Default::default())),
            download_queue: Arc::new(Mutex::new(QueueState::new(0, 0))),
            verification_queue: Arc::new(Mutex::new(Vec::new())),
            verification_queue_size: Arc::new(AtomicUsize::new(0)),
            verification_progress: Arc::new(Mutex::new(Vec::new())),
            download_registry: Arc::new(Mutex::new(crate::registry::load_registry())),
            download_config,
            rate_limiter,
            verification_config: Arc::new(VerificationConfig::new()),
        }
    }

    /// Spawn the background verification worker (drains `verification_queue`,
    /// runs SHA256 checks, updates the registry). Identical for TUI and CLI.
    pub fn spawn_verification_worker(&self) {
        let verification_queue = self.verification_queue.clone();
        let verification_progress = self.verification_progress.clone();
        let verification_queue_size = self.verification_queue_size.clone();
        let status_tx = self.status_tx.clone();
        let download_registry = self.download_registry.clone();
        let verification_config = self.verification_config.clone();
        tokio::spawn(async move {
            crate::verification::verification_worker(
                verification_queue,
                verification_progress,
                verification_queue_size,
                status_tx,
                download_registry,
                verification_config,
            )
            .await;
        });
    }

    /// Spawn the serial download manager used by the TUI.
    ///
    /// Awaits each download to completion inline and decrements the queue
    /// when a download starts.
    pub fn spawn_download_manager_serial(&self) {
        let download_rx = self.download_rx.clone();
        let download_progress = self.download_progress.clone();
        let download_queue = self.download_queue.clone();
        let status_tx = self.status_tx.clone();
        let auth_tx = self.auth_tx.clone();
        let complete_downloads = self.complete_downloads.clone();
        let download_registry = self.download_registry.clone();
        let download_config = self.download_config.clone();
        let rate_limiter = self.rate_limiter.clone();
        let verification_queue = self.verification_queue.clone();
        let verification_queue_size = self.verification_queue_size.clone();

        tokio::spawn(async move {
            loop {
                // Lock only when receiving, release immediately afterwards to
                // respect the documented lock hierarchy.
                let message = {
                    let mut rx = download_rx.lock().await;
                    match rx.recv().await {
                        Some(msg) => msg,
                        None => break, // Channel closed
                    }
                };

                let (_, _, _, _, _, total_size) = &message;
                let total_size = *total_size;

                // Account for the download starting.
                {
                    let mut queue = download_queue.lock().await;
                    queue.remove(1, total_size);
                }

                let params = build_download_params(
                    message,
                    &download_progress,
                    &status_tx,
                    &auth_tx,
                    &complete_downloads,
                    &download_registry,
                    &verification_queue,
                    &verification_queue_size,
                    &download_config,
                    &rate_limiter,
                );

                start_download(params).await;
            }
        });
    }

    /// Spawn the concurrent download manager used by the headless CLI.
    ///
    /// Spawns a task per download (concurrent) and decrements the queue when
    /// a download completes.
    pub fn spawn_download_manager_concurrent(&self) {
        let download_rx = self.download_rx.clone();
        let download_progress = self.download_progress.clone();
        let download_queue = self.download_queue.clone();
        let status_tx = self.status_tx.clone();
        let auth_tx = self.auth_tx.clone();
        let complete_downloads = self.complete_downloads.clone();
        let download_registry = self.download_registry.clone();
        let download_config = self.download_config.clone();
        let rate_limiter = self.rate_limiter.clone();
        let verification_queue = self.verification_queue.clone();
        let verification_queue_size = self.verification_queue_size.clone();

        tokio::spawn(async move {
            loop {
                let message = {
                    let mut rx = download_rx.lock().await;
                    match rx.recv().await {
                        Some(msg) => msg,
                        None => break, // Channel closed
                    }
                };

                let (_, _, _, _, _, total_size) = &message;
                let total_size = *total_size;

                let params = build_download_params(
                    message,
                    &download_progress,
                    &status_tx,
                    &auth_tx,
                    &complete_downloads,
                    &download_registry,
                    &verification_queue,
                    &verification_queue_size,
                    &download_config,
                    &rate_limiter,
                );

                let queue = download_queue.clone();
                tokio::spawn(async move {
                    start_download(params).await;
                    let mut queue = queue.lock().await;
                    queue.remove(1, total_size);
                });
            }
        });
    }
}

impl Default for DownloadRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a [`DownloadParams`] from a download message and the shared handles.
///
/// Shared by both download-manager variants so the field mapping lives in one
/// place.
#[allow(clippy::too_many_arguments)]
fn build_download_params(
    (model_id, filename, base_path, expected_sha256, hf_token, _total_size): DownloadMessage,
    progress: &Arc<Mutex<Option<DownloadProgress>>>,
    status_tx: &mpsc::UnboundedSender<String>,
    auth_tx: &mpsc::UnboundedSender<String>,
    complete_downloads: &Arc<Mutex<CompleteDownloads>>,
    download_registry: &Arc<Mutex<DownloadRegistry>>,
    verification_queue: &Arc<Mutex<Vec<VerificationQueueItem>>>,
    verification_queue_size: &Arc<AtomicUsize>,
    download_config: &Arc<DownloadConfig>,
    rate_limiter: &RateLimiter,
) -> DownloadParams {
    DownloadParams {
        model_id,
        filename,
        base_path,
        progress: progress.clone(),
        status_tx: status_tx.clone(),
        auth_tx: auth_tx.clone(),
        complete_downloads: complete_downloads.clone(),
        download_registry: download_registry.clone(),
        download_config: download_config.clone(),
        rate_limiter: rate_limiter.clone(),
        expected_sha256,
        verification_queue: verification_queue.clone(),
        verification_queue_size: verification_queue_size.clone(),
        hf_token,
    }
}
