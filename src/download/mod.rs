//! Download transport: [`start_download`] orchestrates authenticated,
//! chunked/resumable HTTP downloads with progress reporting.
//! Per-component path sanitization (the path-traversal defense for
//! user-supplied filenames) is owned by [`crate::paths::sanitize`].
//!
//! # Layout
//!
//! This file is a facade over one private submodule (the `engine/`
//! precedent — every `crate::download::X` import keeps compiling
//! unchanged):
//!
//! - `chunked` — [`ChunkedDownloadParams`], `download_chunked` (the
//!   probe/spawn/wait phases, W5.1b), `ChunkContext` +
//!   `download_chunk_with_progress` (the per-chunk worker), and the
//!   chunk-size math.
//!
//! [`start_download`] itself (with its W5.1a phases
//! `prepare_download_paths` / `handle_existing_file` /
//! `execute_download_with_retry`), the retry glue (`is_transient_error`),
//! and the global runtime-tunable configuration
//! ([`DOWNLOAD_CONFIG`]/[`RATE_LIMITER`]) live here in the facade.
//!
//! [`ChunkedDownloadParams`]: chunked::ChunkedDownloadParams

mod chunked;

use crate::models::{CompleteDownloads, DownloadProgress, FileOutcome, VerificationQueueItem};
use crate::paths::sanitize::sanitize_path_component;
use crate::rate_limiter::RateLimiter;
use crate::registry;
use chunked::{download_chunked, ChunkedDownloadParams};
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

/// Parameters for starting a download
pub struct DownloadParams {
    pub model_id: String,
    /// Git revision (branch/tag/SHA) to download from (issue #28).
    pub revision: String,
    pub filename: String,
    pub base_path: PathBuf,
    pub progress: Arc<Mutex<Option<DownloadProgress>>>,
    pub status_tx: mpsc::UnboundedSender<String>,
    pub complete_downloads: Arc<Mutex<CompleteDownloads>>,
    pub expected_sha256: Option<String>,
    pub verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    pub verification_queue_size: Arc<AtomicUsize>,
    pub hf_token: Option<String>,
}

pub async fn start_download(params: DownloadParams) -> FileOutcome {
    let DownloadParams {
        model_id,
        revision,
        filename,
        base_path,
        progress,
        status_tx,
        complete_downloads,
        expected_sha256,
        verification_queue,
        verification_queue_size,
        hf_token,
    } = params;

    // Notify user that download is starting
    let _ = status_tx.send(format!("Starting download: {}", filename));

    let ctx = DownloadCtx {
        model_id: &model_id,
        revision: &revision,
        filename: &filename,
        progress: &progress,
        status_tx: &status_tx,
        complete_downloads: &complete_downloads,
        expected_sha256: &expected_sha256,
        verification_queue: &verification_queue,
        verification_queue_size: &verification_queue_size,
        hf_token: &hf_token,
    };

    // Phase 1: sanitize the filename, create the directory tree, and
    // compute the final/.incomplete paths. An `Err` here is one of the
    // pre-flight `FileOutcome::Failed` returns (invalid filename, dir
    // creation, canonicalization, path traversal) — nothing has been
    // opened yet, so the caller returns it directly (the original early
    // returns cleared no progress state either).
    let prepared = match prepare_download_paths(&ctx, &base_path).await {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };

    // Phase 2: if the final file already exists, mark the registry
    // complete, queue verification, clear progress, and return
    // `AlreadyExists` (the original already-exists early return, cleanup
    // tail included).
    if let Some(outcome) = handle_existing_file(&ctx, &prepared).await {
        return outcome;
    }

    // Phase 3: the retry loop around `download_chunked` (transient-error
    // retries with `.incomplete` deletion, 401 handling, registry
    // completion/failure marking).
    let outcome = execute_download_with_retry(&ctx, &prepared).await;

    // Clear progress when done
    let mut prog = progress.lock().await;
    *prog = None;

    outcome
}

/// Borrowed per-file context the [`start_download`] phases share (W5.1a):
/// everything the destructured [`DownloadParams`] lends the three phases,
/// bundled so each phase signature stays `(ctx, its-specific-inputs)`.
struct DownloadCtx<'a> {
    model_id: &'a str,
    /// Git revision (branch/tag/SHA) the URL points at (issue #28).
    revision: &'a str,
    filename: &'a str,
    progress: &'a Arc<Mutex<Option<DownloadProgress>>>,
    status_tx: &'a mpsc::UnboundedSender<String>,
    complete_downloads: &'a Arc<Mutex<CompleteDownloads>>,
    expected_sha256: &'a Option<String>,
    verification_queue: &'a Arc<Mutex<Vec<VerificationQueueItem>>>,
    verification_queue_size: &'a Arc<AtomicUsize>,
    hf_token: &'a Option<String>,
}

/// The paths and URL [`start_download`]'s post-preparation phases work
/// with (W5.1a).
struct PreparedDownload {
    /// Resolve URL for the (sanitized) filename.
    url: String,
    /// Canonical destination path (under the canonicalized base).
    final_path: PathBuf,
    /// `<final>.incomplete` — the file chunk tasks write into.
    incomplete_path: PathBuf,
}

/// Phase 1 of [`start_download`] (W5.1a): validate the filename against
/// path traversal, create the base/parent directories, canonicalize the
/// base for the containment check, build the final/`.incomplete` paths,
/// and delete any stale `.incomplete` from a previous run
/// (restart-from-beginning semantics). Every `Err` is one of the original
/// pre-flight `FileOutcome::Failed` early returns, status message
/// included; no progress state is cleared on this path (historical
/// behavior).
async fn prepare_download_paths(
    ctx: &DownloadCtx<'_>,
    base_path: &std::path::Path,
) -> Result<PreparedDownload, FileOutcome> {
    let DownloadCtx {
        model_id,
        revision,
        filename,
        status_tx,
        ..
    } = ctx;

    // Validate filename to prevent path traversal
    let sanitized_filename = {
        let parts: Vec<&str> = filename.split('/').collect();
        let mut sanitized_parts = Vec::new();
        for part in parts {
            match sanitize_path_component(part) {
                Some(p) => sanitized_parts.push(p),
                None => {
                    let _ = status_tx.send(format!("Error: Invalid filename component: {}", part));
                    return Err(FileOutcome::Failed {
                        filename: filename.to_string(),
                        reason: format!("invalid filename component: {}", part),
                    });
                }
            }
        }
        sanitized_parts.join("/")
    };

    let url = crate::api::resolve_url(model_id, &sanitized_filename, revision);

    // Create directory if it doesn't exist
    if let Err(e) = tokio::fs::create_dir_all(base_path).await {
        let _ = status_tx.send(format!("Error: Failed to create directory: {}", e));
        return Err(FileOutcome::Failed {
            filename: filename.to_string(),
            reason: format!("failed to create directory: {}", e),
        });
    }

    // Canonicalize base path for safety checks
    let canonical_base = match base_path.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            let _ = status_tx.send(format!("Error: Cannot canonicalize base path: {}", e));
            return Err(FileOutcome::Failed {
                filename: filename.to_string(),
                reason: format!("cannot canonicalize base path: {}", e),
            });
        }
    };

    // Build the final path preserving the directory structure from the filename
    // The filename may contain subdirectories (e.g., "tokenizer/config.json", "Q2_K_L/model.gguf")
    let final_path = canonical_base.join(&sanitized_filename);

    // Ensure final path is still under base directory
    if let Some(parent) = final_path.parent() {
        if let Ok(canonical_final_parent) = parent.canonicalize() {
            if !canonical_final_parent.starts_with(&canonical_base) {
                let _ = status_tx.send("Error: Path traversal detected".to_string());
                return Err(FileOutcome::Failed {
                    filename: filename.to_string(),
                    reason: "path traversal detected".to_string(),
                });
            }
        }
    }

    // Construct file paths
    let incomplete_path = final_path.parent().unwrap_or(&canonical_base).join(format!(
        "{}.incomplete",
        final_path.file_name().unwrap().to_string_lossy()
    ));

    // Create parent directories for the file (in case filename contains subdirectories like "Q4_K_M/file.gguf")
    if let Some(parent) = final_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            let _ = status_tx.send(format!("Error: Failed to create parent directory: {}", e));
            return Err(FileOutcome::Failed {
                filename: filename.to_string(),
                reason: format!("failed to create parent directory: {}", e),
            });
        }
    }
    if let Some(parent) = incomplete_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            let _ = status_tx.send(format!(
                "Error: Failed to create parent directory for incomplete file: {}",
                e
            ));
            return Err(FileOutcome::Failed {
                filename: filename.to_string(),
                reason: format!("failed to create parent directory: {}", e),
            });
        }
    }

    // Check for incomplete downloads and delete them to restart from beginning
    if incomplete_path.exists() {
        let _ = status_tx.send(format!(
            "Found incomplete download for {}, restarting from beginning",
            filename
        ));
        if let Err(e) = tokio::fs::remove_file(&incomplete_path).await {
            let _ = status_tx.send(format!("Warning: Failed to delete incomplete file: {}", e));
        }
    }

    Ok(PreparedDownload {
        url,
        final_path,
        incomplete_path,
    })
}

/// Phase 2 of [`start_download`] (W5.1a): the already-exists branch. When
/// the final file is on disk, mark the registry entry complete, queue
/// verification when enabled and a hash is available, clear the progress
/// slot, and return `Some(AlreadyExists)`; `None` means "not present,
/// proceed to the download". The progress clear is part of the original
/// early return — kept inside so the caller's `return` stays bare.
async fn handle_existing_file(
    ctx: &DownloadCtx<'_>,
    prepared: &PreparedDownload,
) -> Option<FileOutcome> {
    let DownloadCtx {
        filename,
        progress,
        status_tx,
        complete_downloads,
        expected_sha256,
        ..
    } = ctx;
    let PreparedDownload {
        url,
        final_path,
        incomplete_path: _,
    } = prepared;

    // Also check for the complete file - if it exists, queue for verification if enabled
    if !final_path.exists() {
        return None;
    }

    let _ = status_tx.send(format!(
        "File {} already exists, skipping download",
        filename
    ));

    let file_size = tokio::fs::metadata(final_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    // Update registry as complete
    registry::mark_complete(
        complete_downloads,
        registry::Completion::AlreadyExists { url },
        filename,
    )
    .await;

    // Queue verification if enabled AND hash is available
    let verification_enabled = DOWNLOAD_CONFIG.enable_verification.load(Ordering::Relaxed);
    if verification_enabled {
        if let Some(expected_hash) = &expected_sha256 {
            let item = VerificationQueueItem {
                filename: filename.to_string(),
                local_path: final_path.to_string_lossy().to_string(),
                expected_sha256: expected_hash.clone(),
                total_size: file_size,
                is_manual: false,
            };

            crate::verification::queue_verification(
                ctx.verification_queue.clone(),
                ctx.verification_queue_size.clone(),
                item,
            )
            .await;

            let _ = status_tx.send(format!("Queued {} for verification", filename));
        } else {
            let _ = status_tx.send(format!(
                "File {} exists but no hash available for verification",
                filename
            ));
        }
    }

    let mut prog = progress.lock().await;
    *prog = None;
    Some(FileOutcome::AlreadyExists {
        filename: filename.to_string(),
        bytes: file_size,
    })
}

/// Phase 3 of [`start_download`] (W5.1a): the retry loop around
/// [`download_chunked`]. Transient errors (timeout/connection) consume a
/// retry and restart from scratch (the `.incomplete` file is deleted);
/// 401s map to `AuthRequired`; every other terminal error (and retry
/// exhaustion) deletes the `.incomplete` file and marks the registry
/// entry failed before returning. All registry marking and temp-file
/// cleanup lives here so no extracted `?` can skip it.
async fn execute_download_with_retry(
    ctx: &DownloadCtx<'_>,
    prepared: &PreparedDownload,
) -> FileOutcome {
    let DownloadCtx {
        model_id,
        revision,
        filename,
        progress,
        status_tx,
        complete_downloads,
        expected_sha256,
        hf_token,
        ..
    } = ctx;
    let PreparedDownload {
        url,
        final_path,
        incomplete_path,
    } = prepared;

    let mut retries = DOWNLOAD_CONFIG.max_retries.load(Ordering::Relaxed);
    let outcome;
    loop {
        let chunked_params = ChunkedDownloadParams {
            url,
            incomplete_path,
            final_path,
            progress,
            status_tx,
            complete_downloads,
            filename,
            expected_sha256,
            hf_token,
            revision,
        };

        match download_chunked(chunked_params, model_id).await {
            Ok((final_size, expected_size, verification_item, successful_url)) => {
                // Verify the download is complete
                if final_size == expected_size && expected_size > 0 {
                    // Update registry: mark as complete and update URL if it changed (raw fallback)
                    registry::mark_complete(
                        complete_downloads,
                        registry::Completion::Downloaded {
                            url,
                            successful_url: &successful_url,
                            downloaded_size: final_size,
                        },
                        filename,
                    )
                    .await;

                    // Queue verification if enabled AND hash is available
                    let verification_enabled =
                        DOWNLOAD_CONFIG.enable_verification.load(Ordering::Relaxed);
                    if verification_enabled {
                        if let Some(item) = verification_item {
                            crate::verification::queue_verification(
                                ctx.verification_queue.clone(),
                                ctx.verification_queue_size.clone(),
                                item,
                            )
                            .await;
                            let _ = status_tx.send(format!(
                                "Download complete, queued for verification: {}",
                                filename
                            ));
                        } else {
                            let _ = status_tx.send(format!(
                                "Download complete: {} (no hash available)",
                                filename
                            ));
                        }
                    } else {
                        let _ = status_tx.send(format!("Download complete: {}", filename));
                    }
                    outcome = FileOutcome::Complete {
                        filename: filename.to_string(),
                        bytes: final_size,
                    };
                } else {
                    let _ = status_tx.send(format!(
                        "Warning: Download may be incomplete: {} (got {} bytes, expected {})",
                        filename, final_size, expected_size
                    ));
                    outcome = FileOutcome::Failed {
                        filename: filename.to_string(),
                        reason: format!(
                            "incomplete download: got {} bytes, expected {}",
                            final_size, expected_size
                        ),
                    };
                }
                break;
            }
            Err(e) if retries > 0 && is_transient_error(&e) => {
                retries -= 1;
                let _ = status_tx.send(format!(
                    "Download interrupted: {}. Retrying ({} left)...",
                    e, retries
                ));
                let retry_delay = DOWNLOAD_CONFIG.retry_delay_secs.load(Ordering::Relaxed);
                tokio::time::sleep(tokio::time::Duration::from_secs(retry_delay)).await;

                // Delete incomplete file to restart from beginning
                if incomplete_path.exists() {
                    let _ = tokio::fs::remove_file(incomplete_path).await;
                }
                continue;
            }
            Err(e) => {
                // Check for 401 Unauthorized errors
                if let Some(reqwest_err) = e.downcast_ref::<reqwest::Error>() {
                    if reqwest_err.status() == Some(reqwest::StatusCode::UNAUTHORIZED) {
                        let _ = status_tx.send(crate::models::auth_status_message(model_id));

                        // Delete incomplete file
                        if incomplete_path.exists() {
                            let _ = tokio::fs::remove_file(incomplete_path).await;
                        }

                        // Update registry with failed state
                        registry::mark_failed(url);

                        outcome = FileOutcome::AuthRequired {
                            model_id: model_id.to_string(),
                        };
                        break;
                    }
                }

                let _ = status_tx.send(format!("Error: Download failed after retries: {}", e));

                // Delete incomplete file
                if incomplete_path.exists() {
                    let _ = tokio::fs::remove_file(incomplete_path).await;
                }

                // Update registry with failed state
                registry::mark_failed(url);

                outcome = FileOutcome::Failed {
                    filename: filename.to_string(),
                    reason: format!("download failed after retries: {}", e),
                };
                break;
            }
        }
    }

    outcome
}

#[allow(clippy::borrowed_box)]
fn is_transient_error(e: &Box<dyn std::error::Error + Send + Sync>) -> bool {
    // Check if error is a reqwest error and if it's a timeout or connection error
    if let Some(reqwest_err) = e.downcast_ref::<reqwest::Error>() {
        return reqwest_err.is_timeout() || reqwest_err.is_connect();
    }
    false
}

// Global download configuration (thread-safe, runtime-modifiable)
pub struct DownloadConfig {
    pub concurrent_threads: AtomicUsize,
    pub target_chunks: AtomicUsize,
    pub min_chunk_size: AtomicU64,
    pub max_chunk_size: AtomicU64,
    pub enable_verification: AtomicBool,
    pub max_retries: AtomicU32,
    pub download_timeout_secs: AtomicU64,
    pub retry_delay_secs: AtomicU64,
    pub progress_update_interval_ms: AtomicU64,
    pub rate_limit_enabled: AtomicBool,
    pub rate_limit_bytes_per_sec: AtomicU64,
}

impl DownloadConfig {
    pub const fn new() -> Self {
        Self {
            concurrent_threads: AtomicUsize::new(8),
            target_chunks: AtomicUsize::new(20),
            min_chunk_size: AtomicU64::new(5 * 1024 * 1024),
            max_chunk_size: AtomicU64::new(100 * 1024 * 1024),
            enable_verification: AtomicBool::new(true),
            max_retries: AtomicU32::new(5),
            download_timeout_secs: AtomicU64::new(300),
            retry_delay_secs: AtomicU64::new(1),
            progress_update_interval_ms: AtomicU64::new(200),
            rate_limit_enabled: AtomicBool::new(false),
            rate_limit_bytes_per_sec: AtomicU64::new(50 * 1024 * 1024), // 50 MB/s
        }
    }
}

// Global static configuration
pub static DOWNLOAD_CONFIG: DownloadConfig = DownloadConfig::new();

// Global rate limiter instance (initialized lazily)
pub static RATE_LIMITER: Lazy<RateLimiter> = Lazy::new(|| {
    let rate = DOWNLOAD_CONFIG
        .rate_limit_bytes_per_sec
        .load(Ordering::Relaxed);
    RateLimiter::new(rate, 2.0) // 2 second burst window (fixed)
});
