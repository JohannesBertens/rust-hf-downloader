//! Download transport: [`start_download`] orchestrates authenticated,
//! chunked/resumable HTTP downloads with progress reporting.
//! Per-component path sanitization (the path-traversal defense for
//! user-supplied filenames) is owned by [`crate::paths::sanitize`].
//!
//! # Layout
//!
//! This file is a facade over two private submodules (the `engine/`
//! precedent — every `crate::download::X` import keeps compiling
//! unchanged):
//!
//! - `chunked` — [`ChunkedDownloadParams`], `download_chunked` (the
//!   probe/spawn/wait phases, W5.1b), `ChunkContext` +
//!   `download_chunk_with_progress` (the per-chunk worker), and the
//!   chunk-size math.
//! - `journal` — the resume journal (issue #50): the
//!   `<final>.incomplete.state.json` sidecar and its writer (see that
//!   module's docs for the validation rules, crash ordering, and the
//!   terminal-leaf lock discipline); only [`journal_sidecar_path`] is
//!   exported for outside consumers.
//!
//! [`start_download`] itself (with its W5.1a phases
//! `prepare_download_paths` / `handle_existing_file` /
//! `execute_download_with_retry`), the retry glue (`is_transient_error`),
//! and the global runtime-tunable configuration
//! ([`DOWNLOAD_CONFIG`]/[`RATE_LIMITER`]) live here in the facade.
//!
//! [`ChunkedDownloadParams`]: chunked::ChunkedDownloadParams

mod chunked;
mod journal;

/// Path of the resume-journal sidecar for a `.incomplete` file path
/// (issue #50) — the one place the `<path>.state.json` naming scheme is
/// exported for consumers outside the transport (the TUI delete flow);
/// everything under [`journal`] stays private to the facade.
pub fn journal_sidecar_path(incomplete_path: &std::path::Path) -> PathBuf {
    journal::journal_path(incomplete_path)
}

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
    /// The engine's verification hub (queue + counters): a clone sharing
    /// every Arc, so `start_download` queues verification work without
    /// touching the engine module (M3 — the DAG edge is download →
    /// verification, never download → engine).
    pub verification: crate::verification::VerificationHub,
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
        verification,
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
        verification: &verification,
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
    // retries RESUME from the journal — the `.incomplete` file is kept,
    // issue #50; 401 handling, registry completion/failure marking).
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
    verification: &'a crate::verification::VerificationHub,
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
/// reject a symlinked write target, and decide the fate of a pre-existing
/// partial download (issue #50): a
/// `.incomplete` with a resume journal is KEPT for `download_chunked` to
/// validate and resume; one without a journal is deleted
/// (restart-from-beginning semantics for orphans and foreign files).
/// Every `Err` is one of the original pre-flight `FileOutcome::Failed`
/// early returns, status message included — the one addition is the
/// containment check's fail-closed `None` arm, which reuses the traversal
/// rejection; no progress state is cleared on this path (historical
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

    // Containment check (audit fix 1b): walk to the FIRST EXISTING ancestor
    // of the final path — the same rule `sanitize::validate_and_sanitize_path`
    // applies with its own equivalent walk. The helper excludes its argument,
    // so this starts at `final_path.parent()` (the direct-parent check it
    // replaces) and keeps walking up while the subdirectories do not exist
    // yet — which is what a *deep* symlinked subdir used to slip past
    // (`Q4_K_M/sub/file.gguf` with `…/model/Q4_K_M` a symlink elsewhere and
    // `sub` never created).
    //
    // STRICT containment: the ancestor must be under the canonical base, full
    // stop. The `base.starts_with(ancestor)` allowance the sanitize side
    // carried (rev 3, P0) is not needed here — `base_path` was created above,
    // so the base really exists — and it would accept a symlink pointing at
    // an ancestor of the base (`…/model/Q4_K_M` → the temp root), i.e. an
    // arbitrary-location write. A dangling intermediate symlink cannot be
    // canonicalized, so the walk passes at its parent and the rejection
    // happens on the `create_dir_all` below (fail closed, no outside write).
    // Residuals this check cannot reach (a pre-built base_path baked through a
    // planted author/model symlink, the check-then-open TOCTOU, the
    // create_dir_all blast radius): docs/DEFERRED.md#download-author-model-symlink-residual.
    //
    // A `None` from the walk fails closed too (impl-gate P2): the base was
    // created and canonicalized above, so the walk must reach at least the
    // base — finding no existing ancestor at all is an anomaly (the base
    // vanished underneath us, or no component canonicalizes), not a normal
    // case. An unproven containment is a rejected one, exactly like the
    // uninspectable paths in the `symlink_metadata` loop below.
    let Some(canonical_parent) = crate::paths::sanitize::nearest_existing_ancestor(&final_path)
    else {
        let _ = status_tx.send("Error: Path traversal detected".to_string());
        return Err(FileOutcome::Failed {
            filename: filename.to_string(),
            reason: "path traversal detected".to_string(),
        });
    };
    if !canonical_parent.starts_with(&canonical_base) {
        let _ = status_tx.send("Error: Path traversal detected".to_string());
        return Err(FileOutcome::Failed {
            filename: filename.to_string(),
            reason: "path traversal detected".to_string(),
        });
    }

    // Construct file paths
    let incomplete_path = final_path.parent().unwrap_or(&canonical_base).join(format!(
        "{}.incomplete",
        final_path.file_name().unwrap().to_string_lossy()
    ));
    let journal_path = journal::journal_path(&incomplete_path);

    // A symlink AT one of the three write targets escapes the base even when
    // every ancestor check passes (audit fix 1b): the final path would be
    // mistaken for a completed file, the `.incomplete` is the real write
    // target, and a symlinked journal would be treated as resume state.
    // Checked before any mkdir/open. `NotFound` means "nothing there,
    // proceed"; any other error fails closed (an uninspectable path is not a
    // proven-safe one).
    for target in [&final_path, &incomplete_path, &journal_path] {
        match std::fs::symlink_metadata(target) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let _ = status_tx.send("Error: Path traversal detected".to_string());
                return Err(FileOutcome::Failed {
                    filename: filename.to_string(),
                    reason: "path traversal detected".to_string(),
                });
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                let _ = status_tx.send(format!("Error: Cannot inspect download path: {}", e));
                return Err(FileOutcome::Failed {
                    filename: filename.to_string(),
                    reason: format!("cannot inspect download path: {}", e),
                });
            }
        }
    }

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

    // Partial-download handling (issue #50): a `.incomplete` file WITH a
    // resume journal next to it is kept — `download_chunked` validates
    // the journal against the freshly probed file and resumes the missing
    // byte ranges. An `.incomplete` WITHOUT a journal (orphans from older
    // versions or foreign files) is deleted as before, as is an orphan
    // journal whose data file is gone (a journal alone is not progress).
    if incomplete_path.exists() {
        if journal_path.exists() {
            let _ = status_tx.send(format!(
                "Found incomplete download for {}, resuming",
                filename
            ));
        } else {
            let _ = status_tx.send(format!(
                "Found incomplete download for {} without a journal, restarting from beginning",
                filename
            ));
            if let Err(e) = tokio::fs::remove_file(&incomplete_path).await {
                let _ = status_tx.send(format!("Warning: Failed to delete incomplete file: {}", e));
            }
        }
    } else if journal_path.exists() {
        journal::remove(&journal_path);
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
        incomplete_path,
    } = prepared;

    // Also check for the complete file - if it exists, queue for verification if enabled
    if !final_path.exists() {
        return None;
    }

    // The final file is complete: any leftover partial state next to it is
    // stale (fix-review finding — the pre-#50 prepare-time delete used to
    // clear it; a full-size `.incomplete` + journal must not leak forever
    // beside a finished file).
    if incomplete_path.exists() {
        let _ = tokio::fs::remove_file(incomplete_path).await;
    }
    let stale_journal = journal::journal_path(incomplete_path);
    if stale_journal.exists() {
        journal::remove(&stale_journal);
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

            ctx.verification.queue_verification(item).await;

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
/// retry and RESUME from the journal (`download_chunked` re-validates it
/// against its fresh probe and skips already-downloaded ranges — issue
/// #50); 401s map to `AuthRequired`; every other terminal error (and
/// retry exhaustion) PRESERVES the `.incomplete` file + journal for a
/// later run while marking the registry entry failed. All registry
/// marking lives here so no extracted `?` can skip it.
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
                            ctx.verification.queue_verification(item).await;
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
                    "Download interrupted: {}. Retrying ({} left), resuming from previous progress...",
                    e, retries
                ));
                let retry_delay = DOWNLOAD_CONFIG.retry_delay_secs.load(Ordering::Relaxed);
                tokio::time::sleep(tokio::time::Duration::from_secs(retry_delay)).await;

                // Issue #50: the `.incomplete` file and its journal are
                // KEPT — the next attempt resumes the missing byte ranges
                // instead of restarting from byte 0.
                continue;
            }
            Err(e) => {
                // Check for 401 Unauthorized errors
                if let Some(reqwest_err) = e.downcast_ref::<reqwest::Error>() {
                    if reqwest_err.status() == Some(reqwest::StatusCode::UNAUTHORIZED) {
                        let _ = status_tx.send(crate::models::auth_status_message(model_id));

                        // Issue #50: keep `.incomplete` + journal — a
                        // later authenticated run resumes where this one
                        // was cut off.

                        // Update registry with failed state
                        registry::mark_failed(url);

                        outcome = FileOutcome::AuthRequired {
                            model_id: model_id.to_string(),
                        };
                        break;
                    }
                }

                let _ = status_tx.send(format!("Error: Download failed after retries: {}", e));

                // Issue #50: keep `.incomplete` + journal so a later run
                // (retry, re-auth, or full re-run) resumes instead of
                // restarting from byte 0 — the entire point of the issue.

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

#[cfg(all(test, unix))]
mod containment_tests {
    //! Audit fix 1b: the write-time containment of [`prepare_download_paths`].
    //! The first existing ancestor of the final path must be STRICTLY under
    //! the canonical base (a symlink to an ancestor of the base is an escape,
    //! not the not-yet-created-base case), and a symlink at the final path,
    //! the `.incomplete` write target, or the resume-journal sidecar is
    //! rejected before any mkdir/open.
    //!
    //! Unix-only (symlinks). No env vars are touched and the registry is
    //! never written (phase 1 only), so no `ENV_MUTEX` is needed; temp dirs
    //! follow the crate convention (`std::env::temp_dir()` + pid + counter).
    use super::{prepare_download_paths, DownloadCtx, PreparedDownload};
    use crate::models::{CompleteDownloads, DownloadProgress, FileOutcome};
    use crate::verification::{VerificationHub, VerificationResultCounters};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Fresh temp root for one test run.
    fn temp_root(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "rhd-dl-contain-{}-{}-{}",
            tag,
            std::process::id(),
            n
        ))
    }

    /// Run phase 1 with a throwaway context (only `filename` + `base_path`
    /// matter for containment; the channels and mirrors are discarded).
    async fn prepare(filename: &str, base_path: &Path) -> Result<PreparedDownload, FileOutcome> {
        let (status_tx, _status_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress = Arc::new(Mutex::new(None::<DownloadProgress>));
        let complete = Arc::new(Mutex::new(CompleteDownloads::default()));
        let (hub_status_tx, _hub_status_rx) = tokio::sync::mpsc::unbounded_channel();
        let (verify_tx, _verify_rx) = tokio::sync::mpsc::unbounded_channel();
        let verification = VerificationHub {
            queue: Arc::new(Mutex::new(Vec::new())),
            size: Arc::new(AtomicUsize::new(0)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            progress: Arc::new(Mutex::new(Vec::new())),
            results: VerificationResultCounters::default(),
            status_tx: hub_status_tx,
            verify_tx,
            registry_mirror: Arc::new(Mutex::new(crate::models::DownloadRegistry::default())),
        };
        let expected_sha256: Option<String> = None;
        let hf_token: Option<String> = None;
        let ctx = DownloadCtx {
            model_id: "a/b",
            revision: crate::api::DEFAULT_REVISION,
            filename,
            progress: &progress,
            status_tx: &status_tx,
            complete_downloads: &complete,
            expected_sha256: &expected_sha256,
            verification: &verification,
            hf_token: &hf_token,
        };
        prepare_download_paths(&ctx, base_path).await
    }

    /// Assert the phase-1 failure is the traversal outcome (the same
    /// `FileOutcome::Failed` shape the pre-existing checks produce).
    fn assert_traversal(result: Result<PreparedDownload, FileOutcome>) {
        match result {
            Err(FileOutcome::Failed { reason, .. }) => {
                assert_eq!(reason, "path traversal detected", "traversal reason")
            }
            Err(other) => panic!("expected a path-traversal failure, got {other:?}"),
            Ok(_) => panic!("expected a path-traversal failure, prepare succeeded"),
        }
    }

    /// `<root>/dl/author/model` — the pre-built base the TUI/CLI hand the
    /// transport (author/model already appended).
    fn base(root: &Path) -> PathBuf {
        root.join("dl").join("author").join("model")
    }

    #[tokio::test]
    async fn deep_symlinked_subdir_is_rejected() {
        // `…/model/Q4_K_M` is a symlink OUTSIDE the base and `sub` does not
        // exist yet: the old final-parent-only check canonicalized nothing
        // (the parent was missing) and the file landed outside the base.
        let root = temp_root("deep");
        let base = base(&root);
        std::fs::create_dir_all(&base).unwrap();
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, base.join("Q4_K_M")).unwrap();

        assert_traversal(prepare("Q4_K_M/sub/file.gguf", &base).await);
        assert!(
            !outside.join("sub").exists(),
            "no directory may be created through the symlink"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn existing_symlinked_parent_is_rejected() {
        // The case the pre-fix check already caught (the direct parent
        // exists): preserved, not regressed.
        let root = temp_root("exists");
        let base = base(&root);
        let outside = root.join("outside");
        std::fs::create_dir_all(base.join("dir")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::remove_dir(base.join("dir")).unwrap();
        std::os::unix::fs::symlink(&outside, base.join("dir")).unwrap();

        assert_traversal(prepare("dir/file.gguf", &base).await);
        assert!(!outside.join("file.gguf").exists(), "no outside write");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn symlink_pointing_at_an_ancestor_of_the_base_is_rejected() {
        // Rev-3 P0: the dropped `base.starts_with(ancestor)` clause accepted
        // this (the link canonicalized to an ANCESTOR of the base). The
        // target is the temp root, never `/` (rev 3.1, opus N4), so a
        // predicate regression cannot write outside the test sandbox.
        let root = temp_root("ancestor");
        let base = base(&root);
        std::fs::create_dir_all(&base).unwrap();
        std::os::unix::fs::symlink(std::env::temp_dir(), base.join("Q4_K_M")).unwrap();

        assert_traversal(prepare("Q4_K_M/sub/file.gguf", &base).await);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn symlinked_final_path_is_rejected_and_never_treated_as_complete() {
        // Every ancestor check passes — the escape is the last component.
        let root = temp_root("final");
        let base = base(&root);
        std::fs::create_dir_all(&base).unwrap();
        let outside = root.join("outside.bin");
        std::fs::write(&outside, b"elsewhere").unwrap();
        std::os::unix::fs::symlink(&outside, base.join("file.gguf")).unwrap();

        assert_traversal(prepare("file.gguf", &base).await);
        assert_eq!(
            std::fs::read(&outside).unwrap(),
            b"elsewhere",
            "the link target was neither written nor treated as the download"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn symlinked_incomplete_target_is_rejected() {
        // The `.incomplete` path is the real write target — its own case
        // (rev 3.1, opus N3), not covered by the final-path check.
        let root = temp_root("incomplete");
        let base = base(&root);
        std::fs::create_dir_all(&base).unwrap();
        let outside = root.join("outside.bin");
        std::fs::write(&outside, b"elsewhere").unwrap();
        std::os::unix::fs::symlink(&outside, base.join("file.gguf.incomplete")).unwrap();

        assert_traversal(prepare("file.gguf", &base).await);
        assert_eq!(std::fs::read(&outside).unwrap(), b"elsewhere", "no write");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn symlinked_journal_sidecar_is_rejected() {
        // A symlinked resume journal is never trusted as resume state
        // (rev 3.1: its own test, independent of the `.incomplete` case).
        let root = temp_root("journal");
        let base = base(&root);
        std::fs::create_dir_all(&base).unwrap();
        let outside = root.join("outside.json");
        std::fs::write(&outside, b"{}").unwrap();
        std::os::unix::fs::symlink(&outside, base.join("file.gguf.incomplete.state.json")).unwrap();

        assert_traversal(prepare("file.gguf", &base).await);
        assert_eq!(std::fs::read(&outside).unwrap(), b"{}", "no write");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn dangling_intermediate_symlink_fails_closed() {
        // Rev 3.1, opus N2: a dangling link cannot be canonicalized, so the
        // ancestor walk passes at its parent and the rejection comes from
        // `create_dir_all` failing on the symlink (fail closed by side
        // effect — accepted, pinned here). No outside write happens.
        let root = temp_root("dangling");
        let base = base(&root);
        std::fs::create_dir_all(&base).unwrap();
        let missing = root.join("nowhere");
        std::os::unix::fs::symlink(&missing, base.join("Q4_K_M")).unwrap();

        let result = prepare("Q4_K_M/file.gguf", &base).await;
        match result {
            Err(FileOutcome::Failed { reason, .. }) => assert!(
                reason.contains("failed to create parent directory"),
                "expected the fail-closed mkdir error, got {reason:?}"
            ),
            Ok(_) => panic!("dangling symlink must fail closed, prepare succeeded"),
            Err(other) => panic!("dangling symlink must fail closed, got {other:?}"),
        }
        assert!(!missing.exists(), "nothing was created at the link target");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn plain_nested_path_is_still_accepted() {
        // No false rejection: the same deep layout with real directories
        // prepares exactly as before (subdirs created under the base).
        let root = temp_root("plain");
        let base = base(&root);
        std::fs::create_dir_all(&base).unwrap();

        let prepared = match prepare("Q4_K_M/sub/file.gguf", &base).await {
            Ok(prepared) => prepared,
            Err(e) => panic!("a plain nested path must not be rejected, got {e:?}"),
        };
        let canonical_base = base.canonicalize().unwrap();
        assert_eq!(
            prepared.final_path,
            canonical_base.join("Q4_K_M").join("sub").join("file.gguf")
        );
        assert!(prepared.final_path.parent().unwrap().is_dir());
        assert_eq!(
            prepared.incomplete_path.parent().unwrap(),
            prepared.final_path.parent().unwrap()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
