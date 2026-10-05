//! Download orchestration: config -> resolve -> engine bootstrap -> event
//! drain. The bootstrap is one of three production sites; `monitor`/
//! `poll_once` follow the AGENTS.md lock hierarchy.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::args::{apply_rate_limit_overrides, merge_token, valid_model_id, DownloadArgs};
use super::events::{ErrorCode, Event, FileDto, FileStatus, OverallProgress, Summary};
use super::report::Reporter;
use super::resolve::{parse_selector, resolve_files, FileSpec, Selector};
use super::{EXIT_AUTH, EXIT_FAILURE, EXIT_INTERRUPTED, EXIT_OK, EXIT_USAGE};
use crate::engine::{EngineState, ManagerHandle, QueuedDownload};
use crate::models::{FileOutcome, VerifyOutcome};

/// Everything the monitor loop accumulates for the summary and exit code.
#[derive(Default)]
pub(super) struct RunTally {
    pub(super) files: usize,
    pub(super) downloaded: usize,
    pub(super) skipped: usize,
    pub(super) verified: usize,
    pub(super) failed: usize,
    pub(super) hash_mismatch: usize,
    pub(super) total_bytes: u64,
    /// Bytes of fully-fetched files (Complete + AlreadyExists outcomes) —
    /// the base for aggregate run progress (see [`OverallProgress`]).
    pub(super) done_bytes: u64,
    pub(super) auth_required: bool,
    pub(super) failures: Vec<String>,
    pub(super) mismatches: Vec<String>,
    /// Authoritative per-file download outcomes, set from the manager's
    /// join list once it resolves (see [`count_outcomes`]). `run_download`
    /// only needs the counts; the `hf-cache sync` publish gate reads the
    /// per-file detail.
    pub(super) outcomes: Vec<FileOutcome>,
    /// Every verification result drained from `verify_rx`, in arrival
    /// order — the per-file input of the `hf-cache sync` publish gate.
    pub(super) verify_outcomes: Vec<VerifyOutcome>,
}

impl RunTally {
    fn exit_code(&self) -> i32 {
        if self.auth_required {
            EXIT_AUTH
        } else if self.failed > 0 || self.hash_mismatch > 0 {
            EXIT_FAILURE
        } else {
            EXIT_OK
        }
    }
}

pub(super) async fn run_download(args: DownloadArgs) -> i32 {
    let mut reporter = Reporter::new(args.json, args.quiet, args.progress);

    // --- 1. Configuration ------------------------------------------------
    let mut options = crate::config::load_config();
    if let Some(dir) = &args.output {
        options.default_directory = dir.clone();
    }
    apply_rate_limit_overrides(
        &mut options,
        args.rate_limit,
        args.no_rate_limit,
        args.rate_limit_mbps,
    );
    let token = merge_token(
        args.token.clone(),
        std::env::var("HF_TOKEN").ok(),
        options.hf_token.clone(),
    );
    options.hf_token = token.clone();
    crate::config::apply_options(&options);
    if args.no_verify {
        crate::download::DOWNLOAD_CONFIG
            .enable_verification
            .store(false, Ordering::Relaxed);
    }

    // --- 2. Validate usage ------------------------------------------------
    let revision = args
        .revision
        .clone()
        .unwrap_or_else(|| crate::api::DEFAULT_REVISION.to_string());
    if !valid_model_id(&args.model_id) {
        reporter.emit(&Event::error(
            ErrorCode::Usage,
            format!(
                "invalid model ID {:?} — expected \"author/model-name\"",
                args.model_id
            ),
        ));
        return EXIT_USAGE;
    }
    let selector = match parse_selector(&args) {
        Ok(selector) => selector,
        Err(message) => {
            reporter.emit(&Event::error(ErrorCode::Usage, message));
            return EXIT_USAGE;
        }
    };

    // --- 3. Resolve files ---------------------------------------------------
    let metadata =
        match crate::api::fetch_model_metadata(&args.model_id, &revision, token.as_ref()).await {
            Ok(metadata) => metadata,
            Err(e) => {
                let not_found = e.status() == Some(reqwest::StatusCode::NOT_FOUND);
                reporter.emit(&Event::Error {
                    code: if not_found {
                        "not_found".to_string()
                    } else {
                        "network".to_string()
                    },
                    message: format!("failed to fetch model info for {}: {}", args.model_id, e),
                    available: None,
                });
                return if not_found { EXIT_USAGE } else { EXIT_FAILURE };
            }
        };

    // Quantization groups derive (pure) from the recursive tree already
    // fetched with the metadata — no second API round-trip. Issue #25:
    // this now finds GGUFs stored in subdirectories and keeps mmproj
    // files in their own groups.
    let quants = if matches!(selector, Selector::Quant(_)) {
        crate::api::classify_quantizations(&metadata.siblings)
    } else {
        Vec::new()
    };

    let files = match resolve_files(&metadata, &quants, &selector) {
        Ok(files) => files,
        Err(err) => {
            reporter.emit(&Event::Error {
                code: err.code().to_string(),
                message: err.message(),
                available: Some(err.available().iter().map(FileDto::from).collect()),
            });
            return EXIT_USAGE;
        }
    };

    let total_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
    reporter.emit(&Event::Resolved {
        model: args.model_id.clone(),
        files: files.iter().map(FileDto::from).collect(),
        total_bytes,
    });

    // --- 4. Register + queue through the shared engine ---------------------
    let base = options.default_directory.clone();
    let pending: Vec<(String, u64, Option<String>)> = files
        .iter()
        .map(|f| (f.filename.clone(), f.size_bytes, f.sha256.clone()))
        .collect();
    if let Err(message) =
        crate::engine::register_pending(&args.model_id, &revision, &pending, &base)
    {
        reporter.emit(&Event::error(ErrorCode::InvalidPath, message));
        return EXIT_FAILURE;
    }

    let (state, download_tx) = EngineState::new();
    // Load the on-disk registry into the engine mirror (parity with the
    // TUI's startup scan) so verification updates find their entries.
    {
        let mut mirror = state.download_registry.lock().await;
        *mirror = crate::registry::load_registry();
    }
    crate::engine::spawn_verification_worker(state.clone());
    let manager = crate::engine::spawn_manager(state.clone());

    // Model files land under base/author/model-name (same layout as the TUI)
    let parts: Vec<&str> = args.model_id.split('/').collect();
    let model_path = PathBuf::from(&base).join(parts[0]).join(parts[1]);

    // Queue accounting + sends (mirrors the TUI's confirm_download)
    {
        let mut queue = state.download_queue.lock().await;
        queue.add(files.len(), total_bytes);
    }
    {
        let mut items = state.download_queue_items.lock().await;
        for file in &files {
            items.push(crate::models::QueueItemSummary {
                filename: file.filename.clone(),
                total_size: file.size_bytes,
            });
        }
    }
    for file in &files {
        let _ = download_tx.send(QueuedDownload {
            model_id: args.model_id.clone(),
            revision: revision.clone(),
            filename: file.filename.clone(),
            base_path: model_path.clone(),
            expected_sha256: file.sha256.clone(),
            hf_token: token.clone(),
            total_size: file.size_bytes,
        });
    }
    // Dropping the sender closes the channel — the manager drains, then its
    // join handle resolves. This is the deterministic completion signal.
    drop(download_tx);

    // --- 5. Monitor until drained ------------------------------------------
    let mut tally = RunTally {
        files: files.len(),
        total_bytes,
        ..RunTally::default()
    };
    let interrupted = monitor(&state, manager, &files, &mut tally, &mut reporter).await;

    // --- 6. Summary + exit code --------------------------------------------
    let summary = Summary {
        files: tally.files,
        downloaded: tally.downloaded,
        skipped: tally.skipped,
        verified: tally.verified,
        failed: tally.failed,
        hash_mismatch: tally.hash_mismatch,
        total_bytes: tally.total_bytes,
    };
    reporter.emit(&Event::Done {
        summary: summary.clone(),
    });
    reporter.destination_line(&model_path.display().to_string());

    reporter.finish();

    if interrupted {
        reporter.emit(&Event::error(ErrorCode::Interrupted, "interrupted by SIGINT; unfinished files stay registered as incomplete and restart from scratch on the next run".to_string()));
        return EXIT_INTERRUPTED;
    }
    if !tally.failures.is_empty() {
        reporter.emit(&Event::error(
            ErrorCode::DownloadFailed,
            tally.failures.join("; "),
        ));
    }
    if !tally.mismatches.is_empty() {
        reporter.emit(&Event::error(
            ErrorCode::HashMismatch,
            tally.mismatches.join("; "),
        ));
    }
    if tally.auth_required {
        reporter.emit(&Event::error(
            ErrorCode::AuthRequired,
            format!(
                "authentication required for {} (pass --token or set $HF_TOKEN)",
                args.model_id
            ),
        ));
    }

    tally.exit_code()
}

/// Poll shared engine state, render, and wait for deterministic drain.
/// Returns true when interrupted by SIGINT.
pub(super) async fn monitor(
    state: &EngineState,
    manager: ManagerHandle,
    files: &[FileSpec],
    tally: &mut RunTally,
    reporter: &mut Reporter,
) -> bool {
    let count = files.len();
    let index_of: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.filename.as_str(), i + 1))
        .collect();

    let mut seen_download: Option<String> = None;
    let mut seen_verifying: HashSet<String> = HashSet::new();

    let mut join = manager.join;
    let mut ticker = tokio::time::interval(Duration::from_millis(400));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // consume the immediate first tick

    let interrupted = loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break None,
            result = &mut join => {
                // Keep the authoritative outcome list; counting happens
                // after the final channel drain below so streamed outcomes
                // are never double-counted.
                break match result {
                    Ok(outcomes) => Some(Some(outcomes)),
                    // Manager task failed (panicked): keep streamed tallies
                    Err(_) => Some(None),
                };
            }
            _ = ticker.tick() => {
                poll_once(
                    state,
                    count,
                    &index_of,
                    &mut seen_download,
                    &mut seen_verifying,
                    tally,
                    reporter,
                )
                .await;
            }
        }
    };

    let outcomes = match interrupted {
        Some(outcomes) => outcomes,
        None => {
            join.abort();
            return true; // interrupted by SIGINT
        }
    };

    // All downloads drained; every queue_verification call has happened.
    // Wait for the verification worker to run dry (race-free idle signal).
    loop {
        if state.verification_idle() {
            break;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                return true;
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                poll_once(
                    state,
                    count,
                    &index_of,
                    &mut seen_download,
                    &mut seen_verifying,
                    tally,
                    reporter,
                )
                .await;
            }
        }
    }

    // Final drain of everything that arrived during the last tick
    poll_once(
        state,
        count,
        &index_of,
        &mut seen_download,
        &mut seen_verifying,
        tally,
        reporter,
    )
    .await;

    // Authoritative recount of download counters from the manager's full
    // outcome list (verification counters come from the drained channel —
    // every send happens before the in-flight counter drops to zero).
    if let Some(outcomes) = outcomes {
        count_outcomes(&outcomes, tally);
    }

    false
}

/// Non-blocking snapshot of engine state: drain channels, detect new
/// downloads/verifications, emit events.
#[allow(clippy::too_many_arguments)]
pub(super) async fn poll_once(
    state: &EngineState,
    count: usize,
    index_of: &HashMap<&str, usize>,
    seen_download: &mut Option<String>,
    seen_verifying: &mut HashSet<String>,
    tally: &mut RunTally,
    reporter: &mut Reporter,
) {
    // Free-text status lines (human mode only; JSON uses typed events)
    if let Ok(mut rx) = state.status_rx.try_lock() {
        while let Ok(message) = rx.try_recv() {
            reporter.status_line(&message);
        }
    }

    // Streaming per-file outcomes
    if let Ok(mut rx) = state.outcome_rx.try_lock() {
        while let Ok(outcome) = rx.try_recv() {
            apply_outcome_event(&outcome, index_of, count, reporter, tally);
        }
    }

    // Verification starts (new entries in the active-progress list).
    // None = the try_lock snapshot missed — skip the heartbeat that tick
    // rather than print a lock artifact as an in-flight count.
    let mut verifying_active: Option<usize> = None;
    if let Ok(progress) = state.verification_progress.try_lock() {
        verifying_active = Some(progress.len());
        for entry in progress.iter() {
            if seen_verifying.insert(entry.filename.clone()) {
                reporter.emit(&Event::VerificationStart {
                    filename: entry.filename.clone(),
                });
            }
        }
    }

    // Typed verification results
    if let Ok(mut rx) = state.verify_rx.try_lock() {
        while let Ok(outcome) = rx.try_recv() {
            apply_verify_outcome(&outcome, reporter, tally);
        }
    }

    // Download progress (single line for the currently-active file)
    let mut download_active = false;
    if let Ok(guard) = state.download_progress.try_lock() {
        if let Some(progress) = guard.as_ref() {
            download_active = true;
            if seen_download.as_deref() != Some(progress.filename.as_str()) {
                *seen_download = Some(progress.filename.clone());
                reporter.emit(&Event::DownloadStart {
                    filename: progress.filename.clone(),
                    index: index_of
                        .get(progress.filename.as_str())
                        .copied()
                        .unwrap_or(0),
                    count,
                    size_bytes: progress.total,
                });
            }
            let percent = if progress.total > 0 {
                (progress.downloaded as f64 / progress.total as f64) * 100.0
            } else {
                0.0
            };
            // Aggregate view for multi-file runs: finished-file bytes
            // (tally.done_bytes) plus the active file's partial bytes. The
            // engine downloads serially, so the active file's speed is the
            // aggregate speed.
            let overall = if count > 1 {
                Some(OverallProgress {
                    files_done: (tally.downloaded + tally.skipped + tally.failed).min(count),
                    files_total: count,
                    downloaded_bytes: tally.done_bytes + progress.downloaded,
                    total_bytes: tally.total_bytes,
                })
            } else {
                None
            };
            reporter.emit(&Event::Progress {
                filename: progress.filename.clone(),
                downloaded_bytes: progress.downloaded,
                total_bytes: progress.total,
                speed_mbps: progress.speed_mbps,
                percent: (percent * 10.0).round() / 10.0,
                overall,
            });
        }
    }

    // `--progress plain` heartbeat for the verification drain: downloads
    // finished (or between files), SHA256 still hashing — no progress
    // events fire in that phase. Shares the plain throttle window with
    // the download line, so at most one heartbeat every ~10 s total.
    if let Some(active) = verifying_active {
        if !download_active && (active > 0 || !state.verification_idle()) {
            reporter.plain_verification(active, tally.verified);
        }
    }
}

fn apply_outcome_event(
    outcome: &crate::models::FileOutcome,
    index_of: &HashMap<&str, usize>,
    count: usize,
    reporter: &mut Reporter,
    tally: &mut RunTally,
) {
    use crate::models::FileOutcome;
    match outcome {
        FileOutcome::Complete { filename, bytes } => {
            tally.downloaded += 1;
            tally.done_bytes += *bytes;
            reporter.emit(&Event::FileComplete {
                filename: filename.clone(),
                status: FileStatus::Downloaded,
                bytes: *bytes,
            });
        }
        FileOutcome::AlreadyExists { filename, bytes } => {
            tally.skipped += 1;
            tally.done_bytes += *bytes;
            reporter.emit(&Event::FileComplete {
                filename: filename.clone(),
                status: FileStatus::AlreadyExists,
                bytes: *bytes,
            });
        }
        FileOutcome::AuthRequired { model_id } => {
            tally.auth_required = true;
            reporter.emit(&Event::error(
                ErrorCode::AuthRequired,
                format!(
                    "authentication required for {} (pass --token or set $HF_TOKEN)",
                    model_id
                ),
            ));
        }
        FileOutcome::Failed { filename, reason } => {
            tally.failed += 1;
            tally.failures.push(format!("{}: {}", filename, reason));
            reporter.emit(&Event::error(
                ErrorCode::DownloadFailed,
                format!("{}: {}", filename, reason),
            ));
            let _ = (index_of, count);
        }
    }
}

fn count_outcomes(outcomes: &[FileOutcome], tally: &mut RunTally) {
    // The join handle returns the authoritative full list. The monitor loop
    // already counted streamed outcomes in the common case; recount from
    // scratch to stay correct if any events were missed.
    tally.downloaded = 0;
    tally.skipped = 0;
    tally.failed = 0;
    tally.done_bytes = 0;
    tally.auth_required = false;
    tally.failures.clear();
    tally.outcomes = outcomes.to_vec();
    for outcome in outcomes {
        match outcome {
            FileOutcome::Complete { bytes, .. } => {
                tally.downloaded += 1;
                tally.done_bytes += bytes;
            }
            FileOutcome::AlreadyExists { bytes, .. } => {
                tally.skipped += 1;
                tally.done_bytes += bytes;
            }
            FileOutcome::AuthRequired { .. } => tally.auth_required = true,
            FileOutcome::Failed { filename, reason } => {
                tally.failed += 1;
                tally.failures.push(format!("{}: {}", filename, reason));
            }
        }
    }
}

fn apply_verify_outcome(outcome: &VerifyOutcome, reporter: &mut Reporter, tally: &mut RunTally) {
    tally.verify_outcomes.push(outcome.clone());
    match outcome {
        VerifyOutcome::Ok { filename } => {
            tally.verified += 1;
            reporter.emit(&Event::VerificationResult {
                filename: filename.clone(),
                ok: true,
                expected_sha256: None,
                actual_sha256: None,
            });
        }
        VerifyOutcome::Mismatch {
            filename,
            expected_sha256,
            actual_sha256,
        } => {
            tally.hash_mismatch += 1;
            tally.mismatches.push(filename.clone());
            reporter.emit(&Event::VerificationResult {
                filename: filename.clone(),
                ok: false,
                expected_sha256: Some(expected_sha256.clone()),
                actual_sha256: Some(actual_sha256.clone()),
            });
        }
        VerifyOutcome::Error { filename, reason } => {
            reporter.emit(&Event::error(
                ErrorCode::VerificationError,
                format!("{}: {}", filename, reason),
            ));
        }
        VerifyOutcome::Missing { filename } => {
            reporter.emit(&Event::error(
                ErrorCode::VerificationError,
                format!("{}: file not found for verification", filename),
            ));
        }
    }
}
