//! One-shot CLI download mode: `rust-hf-downloader download <MODEL_ID> …`
//!
//! Non-interactive frontend over the shared [`crate::engine`] pipeline,
//! designed for scripts and AI-agent skills:
//!
//! - human progress on **stderr** (single-line rewrites when interactive),
//!   the summary on **stdout**;
//! - `--json` emits stable NDJSON events on **stdout** (progress included,
//!   throttled; the `error` event is always the last line on failure);
//! - deterministic exit codes (see [`EXIT_USAGE`] etc.);
//! - ambiguous selections fail fast with the full structured file list so an
//!   agent can re-invoke with an explicit selector in one round-trip.
//!
//! `hf-cache sync` (plans/hf-cache-sync.md §2/§5.2) reuses the same engine
//! to populate the real HuggingFace hub cache, publishing staged downloads
//! atomically through [`crate::hf_cache`]; `hf-cache path` is the pure
//! path-math scripting helper.
//!
//! The TUI remains the default when the binary is started without a
//! subcommand (see `main.rs`).

use crate::engine::{EngineState, ManagerHandle};
use crate::models::{FileOutcome, VerifyOutcome};
use clap::{Parser, Subcommand};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;

#[cfg(test)]
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

mod update_cmd;

mod args;

#[derive(Parser, Debug)]
#[command(
    name = "rust-hf-downloader",
    version, // from CARGO_PKG_VERSION; pinned by a unit test
    about = "TUI and CLI for downloading HuggingFace models",
    long_about = "TUI and CLI for downloading HuggingFace models.\n\nRun without a subcommand to start the interactive TUI."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// One-shot model download (non-interactive, script/AI-friendly)
    #[command(alias = "dl")]
    Download(DownloadArgs),

    /// Populate the HuggingFace hub cache for offline serving (vLLM,
    /// transformers)
    HfCache(HfCacheArgs),

    /// Search HuggingFace models (query-only; prints a table or a JSON array)
    Search(SearchArgs),

    /// Update the installed binary to the newest release
    #[command(alias = "upgrade")]
    Update(UpdateArgs),
}

mod events;
mod hf_cache_cmd;
mod report;
mod resolve;

use args::ModelDto;
use args::{apply_rate_limit_overrides, merge_token, valid_model_id};
use args::{DownloadArgs, HfCacheArgs, SearchArgs, UpdateArgs};
use events::{Event, FileDto, OverallProgress, Summary};
use hf_cache_cmd::run_hf_cache;
use report::{truncate_path, ProgressMode, Reporter};
use resolve::FileSpec;

#[cfg(test)]
use hf_cache_cmd::{absolute_path, ref_name_for_revision, symlinks_enabled};
#[cfg(test)]
use hf_cache_cmd::{select_sync_files, SelectionMode, SyncSelectionError};
use resolve::{parse_selector, resolve_files, Selector};

#[cfg(test)]
use crate::models::{ModelMetadata, QuantizationGroup};
#[cfg(test)]
use args::HfCacheCommand;
#[cfg(test)]
use args::{parse_rate_limit_mbps, parse_revision};
#[cfg(test)]
use report::{
    format_eta, format_file_progress, format_overall_progress, render_bar,
    verification_heartbeat_line,
};
#[cfg(test)]
use resolve::ResolveError;
use update_cmd::run_update;

// ---------------------------------------------------------------------------
// Exit codes (see plans/add-cli.md §2.4)
// ---------------------------------------------------------------------------

/// All requested files present on disk (downloaded or already existed);
/// verification passed or skipped.
pub const EXIT_OK: i32 = 0;
/// Download failed after retries, or any hash mismatch.
pub const EXIT_FAILURE: i32 = 1;
/// Authentication required (gated repo / bad token).
pub const EXIT_AUTH: i32 = 2;
/// Usage error or resolution ambiguity (`EX_USAGE` convention). clap's own
/// usage errors are routed here too, so `2` stays reserved for auth.
pub const EXIT_USAGE: i32 = 64;
/// `update --check` found a newer release (nothing was installed).
pub const EXIT_UPDATE_AVAILABLE: i32 = 70;
/// `update` downloaded an asset whose SHA256 did not match the manifest.
pub const EXIT_CHECKSUM: i32 = 71;
/// Interrupted by SIGINT.
pub const EXIT_INTERRUPTED: i32 = 130;

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

/// Run a parsed CLI command; returns the process exit code.
pub async fn run(command: Command) -> i32 {
    match command {
        Command::Download(args) => run_download(args).await,
        Command::HfCache(args) => run_hf_cache(args).await,
        Command::Search(args) => run_search(args).await,
        Command::Update(args) => run_update(args).await,
    }
}

/// Everything the monitor loop accumulates for the summary and exit code.
#[derive(Default)]
struct RunTally {
    files: usize,
    downloaded: usize,
    skipped: usize,
    verified: usize,
    failed: usize,
    hash_mismatch: usize,
    total_bytes: u64,
    /// Bytes of fully-fetched files (Complete + AlreadyExists outcomes) —
    /// the base for aggregate run progress (see [`OverallProgress`]).
    done_bytes: u64,
    auth_required: bool,
    failures: Vec<String>,
    mismatches: Vec<String>,
    /// Authoritative per-file download outcomes, set from the manager's
    /// join list once it resolves (see [`count_outcomes`]). `run_download`
    /// only needs the counts; the `hf-cache sync` publish gate reads the
    /// per-file detail.
    outcomes: Vec<FileOutcome>,
    /// Every verification result drained from `verify_rx`, in arrival
    /// order — the per-file input of the `hf-cache sync` publish gate.
    verify_outcomes: Vec<VerifyOutcome>,
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

async fn run_download(args: DownloadArgs) -> i32 {
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
        reporter.emit(&Event::Error {
            code: "usage".to_string(),
            message: format!(
                "invalid model ID {:?} — expected \"author/model-name\"",
                args.model_id
            ),
            available: None,
        });
        return EXIT_USAGE;
    }
    let selector = match parse_selector(&args) {
        Ok(selector) => selector,
        Err(message) => {
            reporter.emit(&Event::Error {
                code: "usage".to_string(),
                message,
                available: None,
            });
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
        reporter.emit(&Event::Error {
            code: "invalid_path".to_string(),
            message,
            available: None,
        });
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
        let _ = download_tx.send((
            args.model_id.clone(),
            revision.clone(),
            file.filename.clone(),
            model_path.clone(),
            file.sha256.clone(),
            token.clone(),
            file.size_bytes,
        ));
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
        reporter.emit(&Event::Error {
            code: "interrupted".to_string(),
            message: "interrupted by SIGINT; unfinished files stay registered as incomplete and restart from scratch on the next run".to_string(),
            available: None,
        });
        return EXIT_INTERRUPTED;
    }
    if !tally.failures.is_empty() {
        reporter.emit(&Event::Error {
            code: "download_failed".to_string(),
            message: tally.failures.join("; "),
            available: None,
        });
    }
    if !tally.mismatches.is_empty() {
        reporter.emit(&Event::Error {
            code: "hash_mismatch".to_string(),
            message: tally.mismatches.join("; "),
            available: None,
        });
    }
    if tally.auth_required {
        reporter.emit(&Event::Error {
            code: "auth_required".to_string(),
            message: format!(
                "authentication required for {} (pass --token or set $HF_TOKEN)",
                args.model_id
            ),
            available: None,
        });
    }

    tally.exit_code()
}

// ---------------------------------------------------------------------------
// Search (query-only: one bounded API call, no engine involvement)
// ---------------------------------------------------------------------------

/// Effective search parameters: explicit flag → config default (the same
/// defaults the TUI's filter toolbar starts with).
fn effective_search_params(
    args: &SearchArgs,
    options: &crate::models::AppOptions,
) -> (
    crate::models::SortField,
    crate::models::SortDirection,
    u64,
    u64,
) {
    (
        args.sort
            .map(Into::into)
            .unwrap_or(options.default_sort_field),
        args.direction
            .map(Into::into)
            .unwrap_or(options.default_sort_direction),
        args.min_downloads.unwrap_or(options.default_min_downloads),
        args.min_likes.unwrap_or(options.default_min_likes),
    )
}

/// Fixed-column human table on stdout; the result count goes to stderr so
/// the table stays pipeable.
fn render_search_table(models: &[ModelDto]) {
    use std::io::Write;
    let id_width = models
        .iter()
        .map(|m| m.id.chars().count())
        .chain(std::iter::once("MODEL ID".len()))
        .max()
        .unwrap()
        .min(48);

    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(
        stdout,
        "{:<id_w$}  {:>10}  {:>7}  UPDATED",
        "MODEL ID",
        "DOWNLOADS",
        "LIKES",
        id_w = id_width
    );
    let _ = writeln!(stdout, "{}", "-".repeat(id_width + 31));
    for m in models {
        let updated = m
            .last_modified
            .as_deref()
            .and_then(|s| s.split('T').next())
            .unwrap_or("-");
        let _ = writeln!(
            stdout,
            "{:<id_w$}  {:>10}  {:>7}  {}",
            truncate_path(&m.id, id_width),
            crate::utils::format_number(m.downloads),
            crate::utils::format_number(m.likes),
            updated,
            id_w = id_width
        );
    }
    let _ = stdout.flush();
    eprintln!("{} model(s)", models.len());
}

async fn run_search(args: SearchArgs) -> i32 {
    let mut reporter = Reporter::new(args.json, false, ProgressMode::Auto);
    let options = crate::config::load_config();
    let token = merge_token(
        args.token.clone(),
        std::env::var("HF_TOKEN").ok(),
        options.hf_token.clone(),
    );

    let (sort, direction, min_downloads, min_likes) = effective_search_params(&args, &options);

    match crate::api::fetch_models_filtered(
        &args.query,
        sort,
        direction,
        min_downloads,
        min_likes,
        args.limit,
        token.as_ref(),
    )
    .await
    {
        Ok(models) => {
            let dtos: Vec<ModelDto> = models.iter().map(ModelDto::from).collect();
            if args.json {
                // Queries emit one JSON document (an array), not NDJSON
                // events — events are for streaming pipelines. On failure the
                // only stdout output is a single error event (see below).
                let mut stdout = std::io::stdout().lock();
                match serde_json::to_string_pretty(&dtos) {
                    Ok(json) => {
                        let _ = writeln!(stdout, "{}", json);
                        let _ = stdout.flush();
                    }
                    Err(e) => {
                        drop(stdout);
                        reporter.emit(&Event::Error {
                            code: "internal".to_string(),
                            message: format!("failed to serialize results: {}", e),
                            available: None,
                        });
                        return EXIT_FAILURE;
                    }
                }
            } else if dtos.is_empty() {
                // A successful query with zero hits is still success (exit 0);
                // scripts distinguish via the empty array / table absence.
                eprintln!("No models found.");
            } else {
                render_search_table(&dtos);
            }
            EXIT_OK
        }
        Err(e) => {
            reporter.emit(&Event::Error {
                code: "network".to_string(),
                message: format!("search failed: {}", e),
                available: None,
            });
            EXIT_FAILURE
        }
    }
}

/// Poll shared engine state, render, and wait for deterministic drain.
/// Returns true when interrupted by SIGINT.
async fn monitor(
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
async fn poll_once(
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
                status: "downloaded",
                bytes: *bytes,
            });
        }
        FileOutcome::AlreadyExists { filename, bytes } => {
            tally.skipped += 1;
            tally.done_bytes += *bytes;
            reporter.emit(&Event::FileComplete {
                filename: filename.clone(),
                status: "already_exists",
                bytes: *bytes,
            });
        }
        FileOutcome::AuthRequired { model_id } => {
            tally.auth_required = true;
            reporter.emit(&Event::Error {
                code: "auth_required".to_string(),
                message: format!(
                    "authentication required for {} (pass --token or set $HF_TOKEN)",
                    model_id
                ),
                available: None,
            });
        }
        FileOutcome::Failed { filename, reason } => {
            tally.failed += 1;
            tally.failures.push(format!("{}: {}", filename, reason));
            reporter.emit(&Event::Error {
                code: "download_failed".to_string(),
                message: format!("{}: {}", filename, reason),
                available: None,
            });
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
            reporter.emit(&Event::Error {
                code: "verification_error".to_string(),
                message: format!("{}: {}", filename, reason),
                available: None,
            });
        }
        VerifyOutcome::Missing { filename } => {
            reporter.emit(&Event::Error {
                code: "verification_error".to_string(),
                message: format!("{}: file not found for verification", filename),
                available: None,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn file_spec(filename: &str, size: u64) -> FileSpec {
        FileSpec {
            filename: filename.to_string(),
            size_bytes: size,
            sha256: None,
        }
    }

    fn metadata_with(files: &[(&str, Option<u64>)]) -> ModelMetadata {
        ModelMetadata {
            model_id: "a/b".to_string(),
            library_name: None,
            pipeline_tag: None,
            card_data: None,
            siblings: files
                .iter()
                .map(|(name, size)| crate::models::RepoFile {
                    rfilename: name.to_string(),
                    size: *size,
                    oid: None,
                    lfs: None,
                })
                .collect(),
            tags: Vec::new(),
            sha: None,
        }
    }

    // --- resolve_files -----------------------------------------------------

    #[test]
    fn resolve_default_single_file_repo() {
        let metadata = metadata_with(&[("only.gguf", Some(10))]);
        let files = resolve_files(&metadata, &[], &Selector::Default).unwrap();
        assert_eq!(files, vec![file_spec("only.gguf", 10)]);
    }

    #[test]
    fn resolve_default_multi_file_repo_is_ambiguous_with_list() {
        let metadata = metadata_with(&[("a.gguf", Some(10)), ("b.gguf", Some(20))]);
        let err = resolve_files(&metadata, &[], &Selector::Default).unwrap_err();
        match err {
            ResolveError::Ambiguous { available } => {
                assert_eq!(available.len(), 2);
                assert_eq!(available[1].filename, "b.gguf");
            }
            other => panic!("expected Ambiguous, got {:?}", other),
        }
    }

    #[test]
    fn resolve_quant_case_insensitive_from_groups() {
        let metadata = metadata_with(&[]);
        let quants = vec![QuantizationGroup {
            quant_type: "Q4_K_M".to_string(),
            files: vec![
                crate::models::QuantizationInfo {
                    quant_type: "Q4_K_M".to_string(),
                    filename: "m-00001-of-00002.gguf".to_string(),
                    size: 1,
                    sha256: Some("dead".to_string()),
                },
                crate::models::QuantizationInfo {
                    quant_type: "Q4_K_M".to_string(),
                    filename: "m-00002-of-00002.gguf".to_string(),
                    size: 2,
                    sha256: None,
                },
            ],
            total_size: 3,
        }];
        let files =
            resolve_files(&metadata, &quants, &Selector::Quant("q4_k_m".to_string())).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].sha256.as_deref(), Some("dead"));
        // all parts of the quantization are selected
        assert_eq!(files[1].filename, "m-00002-of-00002.gguf");
    }

    #[test]
    fn resolve_quant_miss_lists_available() {
        let metadata = metadata_with(&[("a.gguf", Some(10))]);
        let err = resolve_files(&metadata, &[], &Selector::Quant("Q8_0".to_string())).unwrap_err();
        assert!(matches!(err, ResolveError::NoFilesMatch { .. }));
        assert_eq!(err.code(), "no_files_match");
    }

    #[test]
    fn resolve_quant_mmproj_selects_all_projector_groups() {
        // Issue #25: `--quant mmproj` spans every MMPROJ* group; exact names
        // (MMPROJ-Q8_0) still match directly and never pull weight files in.
        let metadata = metadata_with(&[
            ("model.Q8_0.gguf", Some(10)),
            ("model.mmproj-Q8_0.gguf", Some(2)),
            ("mmproj-F32.gguf", Some(3)),
        ]);
        let quants = crate::api::classify_quantizations(&metadata.siblings);

        let files =
            resolve_files(&metadata, &quants, &Selector::Quant("mmproj".to_string())).unwrap();
        assert_eq!(
            files,
            vec![
                file_spec("mmproj-F32.gguf", 3),
                file_spec("model.mmproj-Q8_0.gguf", 2)
            ]
        );

        let files = resolve_files(
            &metadata,
            &quants,
            &Selector::Quant("MMPROJ-Q8_0".to_string()),
        )
        .unwrap();
        assert_eq!(files, vec![file_spec("model.mmproj-Q8_0.gguf", 2)]);
    }

    #[test]
    fn resolve_files_exact_and_missing() {
        let metadata = metadata_with(&[("a.gguf", Some(10)), ("b.gguf", Some(20))]);
        let files =
            resolve_files(&metadata, &[], &Selector::Files(vec!["b.gguf".to_string()])).unwrap();
        assert_eq!(files, vec![file_spec("b.gguf", 20)]);

        let err = resolve_files(
            &metadata,
            &[],
            &Selector::Files(vec!["nope.gguf".to_string()]),
        )
        .unwrap_err();
        assert_eq!(err.code(), "no_files_match");
        assert_eq!(err.available().len(), 2);
    }

    #[test]
    fn resolve_files_dedups_repeatable_selectors() {
        let metadata = metadata_with(&[("a.gguf", Some(10))]);
        let files = resolve_files(
            &metadata,
            &[],
            &Selector::Files(vec!["a.gguf".to_string(), "a.gguf".to_string()]),
        )
        .unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn resolve_all_skips_directories_and_unsized() {
        let metadata = metadata_with(&[
            ("a.gguf", Some(10)),
            ("subdir/", None), // directory marker
            ("broken", None),  // no size → skipped
        ]);
        let files = resolve_files(&metadata, &[], &Selector::All).unwrap();
        assert_eq!(files, vec![file_spec("a.gguf", 10)]);
    }

    // --- selectors / args ---------------------------------------------------

    fn download_args(quant: Option<&str>, file: &[&str], all: bool) -> DownloadArgs {
        DownloadArgs {
            model_id: "a/b".to_string(),
            quant: quant.map(String::from),
            file: file.iter().map(|f| f.to_string()).collect(),
            all,
            progress: ProgressMode::Auto,
            output: None,
            token: None,
            no_verify: false,
            json: false,
            quiet: false,
            rate_limit: false,
            no_rate_limit: false,
            rate_limit_mbps: None,
            revision: None,
        }
    }

    #[test]
    fn selector_default_when_none_given() {
        assert_eq!(
            parse_selector(&download_args(None, &[], false)).unwrap(),
            Selector::Default
        );
    }

    #[test]
    fn selector_conflicts_rejected() {
        assert!(parse_selector(&download_args(Some("Q4"), &[], true)).is_err());
        assert!(parse_selector(&download_args(None, &["a.gguf"], true)).is_err());
        assert!(parse_selector(&download_args(Some("Q4"), &["a.gguf"], false)).is_err());
    }

    #[test]
    fn model_id_validation() {
        assert!(valid_model_id("a/b"));
        assert!(!valid_model_id("a"));
        assert!(!valid_model_id("a/b/c"));
        assert!(!valid_model_id("/b"));
        assert!(!valid_model_id("a/"));
        assert!(!valid_model_id(""));
    }

    #[test]
    fn token_precedence_flag_env_then_file() {
        let f = Some("flag".to_string());
        let e = Some("env".to_string());
        let c = Some("cfg".to_string());
        assert_eq!(
            merge_token(f.clone(), e.clone(), c.clone()).as_deref(),
            Some("flag")
        );
        assert_eq!(
            merge_token(None, e.clone(), c.clone()).as_deref(),
            Some("env")
        );
        assert_eq!(merge_token(None, None, c.clone()).as_deref(), Some("cfg"));
        assert_eq!(merge_token(None, None, None), None);
        // empty strings are treated as absent
        assert_eq!(
            merge_token(Some(String::new()), e, c).as_deref(),
            Some("env")
        );
    }

    // --- CLI parsing --------------------------------------------------------

    // --- Rate-limit overrides (issue #26) ---------------------------------

    #[test]
    fn rate_limit_overrides() {
        let mut options = crate::models::AppOptions::default();
        assert!(!options.download_rate_limit_enabled); // default: off
        assert_eq!(options.download_rate_limit_mbps, 50.0);

        // --rate-limit-mbps implies enable and sets the rate
        apply_rate_limit_overrides(&mut options, false, false, Some(12.5));
        assert!(options.download_rate_limit_enabled);
        assert_eq!(options.download_rate_limit_mbps, 12.5);

        // --no-rate-limit disables again (rate stays but is unused)
        apply_rate_limit_overrides(&mut options, false, true, None);
        assert!(!options.download_rate_limit_enabled);

        // config-enabled survives when no flags are passed
        options.download_rate_limit_enabled = true;
        options.download_rate_limit_mbps = 42.0;
        apply_rate_limit_overrides(&mut options, false, false, None);
        assert!(options.download_rate_limit_enabled);
        assert_eq!(options.download_rate_limit_mbps, 42.0);

        // --rate-limit alone enables with the configured rate
        options.download_rate_limit_enabled = false;
        apply_rate_limit_overrides(&mut options, true, false, None);
        assert!(options.download_rate_limit_enabled);
        assert_eq!(options.download_rate_limit_mbps, 42.0);
    }

    #[test]
    fn rate_limit_mbps_parser_rejects_bad_values() {
        assert_eq!(parse_rate_limit_mbps("10").ok(), Some(10.0));
        assert_eq!(parse_rate_limit_mbps("10.5").ok(), Some(10.5));
        assert!(parse_rate_limit_mbps("0").is_err());
        assert!(parse_rate_limit_mbps("-3").is_err());
        assert!(parse_rate_limit_mbps("abc").is_err());
        assert!(parse_rate_limit_mbps("inf").is_err());
        assert!(parse_rate_limit_mbps("NaN").is_err());
    }

    #[test]
    fn progress_mode_flag_parses_and_defaults() {
        let args = Cli::try_parse_from(["hf-downloader", "download", "a/b"]).unwrap();
        let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
            panic!("expected download subcommand");
        };
        assert_eq!(args.progress, ProgressMode::Auto);

        // explicit modes on download
        for (raw, mode) in [
            ("auto", ProgressMode::Auto),
            ("plain", ProgressMode::Plain),
            ("none", ProgressMode::None),
        ] {
            let args = Cli::try_parse_from(["hf-downloader", "download", "a/b", "--progress", raw])
                .unwrap();
            let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
                panic!("expected download subcommand");
            };
            assert_eq!(args.progress, mode, "--progress {raw}");
        }

        // hf-cache sync accepts it too
        let args = Cli::try_parse_from([
            "hf-downloader",
            "hf-cache",
            "sync",
            "a/b",
            "--progress",
            "plain",
        ])
        .unwrap();
        let crate::cli::Command::HfCache(hf) = args.command.expect("subcommand") else {
            panic!("expected hf-cache subcommand");
        };
        let crate::cli::HfCacheCommand::Sync(args) = hf.command else {
            panic!("expected hf-cache sync subcommand");
        };
        assert_eq!(args.progress, ProgressMode::Plain);

        // unknown mode is a usage error
        assert!(
            Cli::try_parse_from(["hf-downloader", "download", "a/b", "--progress", "sparkly"])
                .is_err()
        );
    }

    #[test]
    fn format_file_progress_layout() {
        let line = format_file_progress("model-Q4_K_M.gguf", 1_073_741_824, 2_147_483_648, 100.0);
        // 50% of a 20-cell bar, sizes in GiB units, eta from the remaining
        // 1 GiB at 100 MiB/s = 10.24s → 10s
        assert_eq!(
            line,
            "model-Q4_K_M.gguf 50% [██████████░░░░░░░░░░] 1.00 GB/2.00 GB 100.0 MB/s eta 10s"
        );
    }

    #[test]
    fn format_overall_progress_layout_and_clamp() {
        let overall = OverallProgress {
            files_done: 3,
            files_total: 17,
            downloaded_bytes: 10_485_760,
            total_bytes: 84_102_439_308,
        };
        let line = format_overall_progress(
            "model-00004-of-00017.safetensors",
            2_900_000_000,
            4_947_802_324,
            &overall,
            88.0,
        );
        assert_eq!(
            line,
            "[3/17 files 0% │ 10.00 MB/78.33 GB │ 88.0 MB/s eta 15m11s] ▸ model-00004-of-00017.safetensors 59%"
        );

        // Actual bytes exceeding the tree-reported total clamp at 100%
        // (Content-Range vs tree size), not 104%.
        let over = OverallProgress {
            files_done: 2,
            files_total: 2,
            downloaded_bytes: 104_857_600,
            total_bytes: 102_760_448,
        };
        let line = format_overall_progress("f.bin", 104_857_600, 102_760_448, &over, 5.0);
        assert!(line.starts_with("[2/2 files 100% │ 100.00 MB/98.00 MB"));
    }

    #[test]
    fn verification_heartbeat_line_format() {
        assert_eq!(
            verification_heartbeat_line(2, 41),
            "verifying: 2 in flight, 41 verified"
        );
    }

    #[test]
    fn rate_limit_flags_parse() {
        let args = Cli::try_parse_from([
            "hf-downloader",
            "download",
            "a/b",
            "--rate-limit-mbps",
            "7.25",
        ])
        .unwrap();
        let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
            panic!("expected download subcommand");
        };
        assert_eq!(args.rate_limit_mbps, Some(7.25));
        assert!(!args.rate_limit);
        assert!(!args.no_rate_limit);

        // --rate-limit and --no-rate-limit conflict
        assert!(Cli::try_parse_from([
            "hf-downloader",
            "download",
            "a/b",
            "--rate-limit",
            "--no-rate-limit",
        ])
        .is_err());

        // --rate-limit-mbps and --no-rate-limit conflict
        assert!(Cli::try_parse_from([
            "hf-downloader",
            "download",
            "a/b",
            "--rate-limit-mbps",
            "5",
            "--no-rate-limit",
        ])
        .is_err());
    }

    // --- --revision (issue #28) -------------------------------------------

    #[test]
    fn revision_parser_accepts_branches_tags_and_shas() {
        assert_eq!(parse_revision("main").ok(), Some("main".to_string()));
        assert_eq!(parse_revision("2.0bpw").ok(), Some("2.0bpw".to_string()));
        // slash-separated branch names are legal
        assert_eq!(
            parse_revision("release/1.0").ok(),
            Some("release/1.0".to_string())
        );
        assert_eq!(
            parse_revision("0123456789abcdef").ok(),
            Some("0123456789abcdef".to_string())
        );

        // rejections: empty, traversal, slashes at the edges, whitespace
        assert!(parse_revision("").is_err());
        assert!(parse_revision("..").is_err());
        assert!(parse_revision("a/../b").is_err());
        assert!(parse_revision("/main").is_err());
        assert!(parse_revision("main/").is_err());
        assert!(parse_revision("two words").is_err());
        assert!(parse_revision("ta\tb").is_err());
    }

    #[test]
    fn revision_flag_parses_into_args() {
        let args =
            Cli::try_parse_from(["hf-downloader", "download", "a/b", "--revision", "2.0bpw"])
                .unwrap();
        let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
            panic!("expected download subcommand");
        };
        assert_eq!(args.revision.as_deref(), Some("2.0bpw"));

        // absent → None (engine defaults to main)
        let args = Cli::try_parse_from(["hf-downloader", "download", "a/b"]).unwrap();
        let crate::cli::Command::Download(args) = args.command.expect("subcommand") else {
            panic!("expected download subcommand");
        };
        assert_eq!(args.revision, None);

        // traversal is rejected at the parser level
        assert!(
            Cli::try_parse_from(["hf-downloader", "download", "a/b", "--revision", "..",]).is_err()
        );
    }

    #[test]
    fn version_string_matches_cargo_pkg_version() {
        // v1 CLI drifted here (stale hardcoded version) — never again.
        let err = Cli::try_parse_from(["rust-hf-downloader", "--version"]).unwrap_err();
        assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn parses_download_subcommand_flags() {
        let cli = Cli::try_parse_from([
            "rust-hf-downloader",
            "download",
            "org/model",
            "--quant",
            "Q4_K_M",
            "--file",
            "a.gguf",
            "--file",
            "b.gguf",
            "-o",
            "/tmp/x",
            "--no-verify",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Download(args)) => {
                assert_eq!(args.model_id, "org/model");
                assert_eq!(args.quant.as_deref(), Some("Q4_K_M"));
                assert_eq!(args.file, vec!["a.gguf", "b.gguf"]);
                assert_eq!(args.output.as_deref(), Some("/tmp/x"));
                assert!(args.no_verify);
                assert!(args.json);
            }
            other => panic!("expected download, got {:?}", other),
        }
    }

    #[test]
    fn no_subcommand_means_tui() {
        let cli = Cli::try_parse_from(["rust-hf-downloader"]).unwrap();
        assert!(cli.command.is_none());
    }

    // --- search --------------------------------------------------------------

    #[test]
    fn search_sort_flags_map_to_shared_enums() {
        // The v1 CLI died of drift: --sort was accepted and ignored. These
        // mappings are the boundary — if the flag stops reaching the shared
        // enum, this test fails.
        for (flag, expected) in [
            ("downloads", crate::models::SortField::Downloads),
            ("likes", crate::models::SortField::Likes),
            ("modified", crate::models::SortField::Modified),
            ("name", crate::models::SortField::Name),
        ] {
            let cli = Cli::try_parse_from(["hfd", "search", "q", "--sort", flag]).unwrap();
            match cli.command {
                Some(Command::Search(args)) => {
                    let mapped: crate::models::SortField = args.sort.unwrap().into();
                    assert_eq!(mapped, expected, "--sort {}", flag);
                }
                other => panic!("expected search, got {:?}", other),
            }
        }
    }

    #[test]
    fn search_direction_accepts_asc_desc_aliases() {
        for (flag, expected) in [
            ("asc", crate::models::SortDirection::Ascending),
            ("ascending", crate::models::SortDirection::Ascending),
            ("desc", crate::models::SortDirection::Descending),
            ("descending", crate::models::SortDirection::Descending),
        ] {
            let cli = Cli::try_parse_from(["hfd", "search", "q", "--direction", flag]).unwrap();
            match cli.command {
                Some(Command::Search(args)) => {
                    let mapped: crate::models::SortDirection = args.direction.unwrap().into();
                    assert_eq!(mapped, expected, "--direction {}", flag);
                }
                other => panic!("expected search, got {:?}", other),
            }
        }
    }

    #[test]
    fn search_rejects_invalid_enum_values_and_limit_range() {
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--sort", "bogus"]).is_err());
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--direction", "up"]).is_err());
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "0"]).is_err());
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "501"]).is_err());
        // boundaries pass
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "1"]).is_ok());
        assert!(Cli::try_parse_from(["hfd", "search", "q", "--limit", "500"]).is_ok());
    }

    #[test]
    fn search_params_flag_beats_config_default() {
        let options = crate::models::AppOptions {
            default_sort_field: crate::models::SortField::Name,
            default_sort_direction: crate::models::SortDirection::Ascending,
            default_min_downloads: 100,
            default_min_likes: 10,
            ..crate::models::AppOptions::default()
        };

        // no flags → config defaults
        let args = Cli::try_parse_from(["hfd", "search", "q"]).unwrap();
        let Command::Search(args) = args.command.unwrap() else {
            panic!()
        };
        assert_eq!(
            effective_search_params(&args, &options),
            (
                crate::models::SortField::Name,
                crate::models::SortDirection::Ascending,
                100,
                10
            )
        );

        // explicit flags override
        let args = Cli::try_parse_from([
            "hfd",
            "search",
            "q",
            "--sort",
            "likes",
            "--direction",
            "desc",
            "--min-downloads",
            "5",
            "--min-likes",
            "0",
        ])
        .unwrap();
        let Command::Search(args) = args.command.unwrap() else {
            panic!()
        };
        assert_eq!(
            effective_search_params(&args, &options),
            (
                crate::models::SortField::Likes,
                crate::models::SortDirection::Descending,
                5,
                0
            )
        );
    }

    #[test]
    fn search_dto_preserves_fields() {
        let info = crate::models::ModelInfo {
            id: "a/b".to_string(),
            author: Some("a".to_string()),
            downloads: 1500,
            likes: 10,
            tags: vec!["gguf".to_string()],
            last_modified: Some("2026-08-01T12:00:00Z".to_string()),
        };
        let dto = ModelDto::from(&info);
        assert_eq!(dto.id, "a/b");
        assert_eq!(dto.author.as_deref(), Some("a"));
        assert_eq!(dto.downloads, 1500);
        assert_eq!(dto.likes, 10);
        assert_eq!(dto.tags, vec!["gguf".to_string()]);
        assert_eq!(dto.last_modified.as_deref(), Some("2026-08-01T12:00:00Z"));
    }

    #[test]
    fn snapshot_search_json_array() {
        let dtos = vec![ModelDto {
            id: "bartowski/Qwen2.5-7B-GGUF".to_string(),
            author: Some("bartowski".to_string()),
            downloads: 123_456,
            likes: 1_200,
            last_modified: Some("2026-08-14T10:00:00Z".to_string()),
            tags: vec!["gguf".to_string(), "text-generation".to_string()],
        }];
        insta::assert_snapshot!(
            "search-result-array",
            serde_json::to_string_pretty(&dtos).unwrap()
        );
    }

    // --- rendering helpers --------------------------------------------------

    #[test]
    fn bar_and_eta_render() {
        assert_eq!(render_bar(0, 10), format!("[{}]", "░".repeat(20)));
        assert_eq!(
            render_bar(5, 10),
            format!("[{}{}]", "█".repeat(10), "░".repeat(10))
        );
        assert_eq!(render_bar(10, 10), format!("[{}]", "█".repeat(20)));
        assert_eq!(format_eta(59.4), "59s");
        assert_eq!(format_eta(95.0), "1m35s");
        assert_eq!(format_eta(3700.0), "1h1m");
    }

    #[test]
    fn truncate_keeps_tail() {
        assert_eq!(truncate_path("short.gguf", 20), "short.gguf");
        let long = "author/model-name/subdir/file-Q4_K_M.gguf";
        let cut = truncate_path(long, 20);
        assert!(cut.starts_with('…'));
        assert!(cut.ends_with("Q4_K_M.gguf"));
        assert_eq!(cut.chars().count(), 20);
    }

    // --- JSON event schema snapshots (insta) --------------------------------

    fn snap(event: &Event, name: &str) {
        let json = serde_json::to_string_pretty(event).unwrap();
        insta::assert_snapshot!(name, json);
    }

    #[test]
    fn snapshot_event_resolved() {
        snap(
            &Event::Resolved {
                model: "org/model".to_string(),
                files: vec![FileDto {
                    filename: "model-Q4_K_M.gguf".to_string(),
                    size_bytes: 4_947_802_324,
                    sha256: Some("a".repeat(64)),
                }],
                total_bytes: 4_947_802_324,
            },
            "event-resolved",
        );
    }

    #[test]
    fn snapshot_event_progress() {
        snap(
            &Event::Progress {
                filename: "model-Q4_K_M.gguf".to_string(),
                downloaded_bytes: 1_048_576,
                total_bytes: 4_947_802_324,
                speed_mbps: 62.4,
                percent: 0.021_183,
                overall: None,
            },
            "event-progress",
        );
    }

    #[test]
    fn snapshot_event_progress_overall() {
        snap(
            &Event::Progress {
                filename: "model-00003-of-00017.safetensors".to_string(),
                downloaded_bytes: 1_048_576,
                total_bytes: 4_947_802_324,
                speed_mbps: 62.4,
                percent: 0.021_183,
                overall: Some(OverallProgress {
                    files_done: 2,
                    files_total: 17,
                    downloaded_bytes: 10_485_760,
                    total_bytes: 84_102_439_308,
                }),
            },
            "event-progress-overall",
        );
    }

    #[test]
    fn snapshot_event_file_complete() {
        snap(
            &Event::FileComplete {
                filename: "model-Q4_K_M.gguf".to_string(),
                status: "downloaded",
                bytes: 4_947_802_324,
            },
            "event-file-complete",
        );
    }

    #[test]
    fn snapshot_event_verification_result_ok_omits_hashes() {
        snap(
            &Event::VerificationResult {
                filename: "model-Q4_K_M.gguf".to_string(),
                ok: true,
                expected_sha256: None,
                actual_sha256: None,
            },
            "event-verification-ok",
        );
    }

    #[test]
    fn snapshot_event_verification_result_mismatch_includes_hashes() {
        snap(
            &Event::VerificationResult {
                filename: "model-Q4_K_M.gguf".to_string(),
                ok: false,
                expected_sha256: Some("e".repeat(64)),
                actual_sha256: Some("a".repeat(64)),
            },
            "event-verification-mismatch",
        );
    }

    #[test]
    fn snapshot_event_done() {
        snap(
            &Event::Done {
                summary: Summary {
                    files: 3,
                    downloaded: 2,
                    skipped: 1,
                    verified: 3,
                    failed: 0,
                    hash_mismatch: 0,
                    total_bytes: 4_947_802_324,
                },
            },
            "event-done",
        );
    }

    #[test]
    fn snapshot_event_error_ambiguous_lists_available() {
        snap(
            &Event::Error {
                code: "ambiguous".to_string(),
                message: "model has 2 downloadable file(s); specify --quant <TYPE>, --file <PATH>, or --all".to_string(),
                available: Some(vec![
                    FileDto {
                        filename: "model-Q4_K_M.gguf".to_string(),
                        size_bytes: 1,
                        sha256: None,
                    },
                    FileDto {
                        filename: "model-Q8_0.gguf".to_string(),
                        size_bytes: 2,
                        sha256: None,
                    },
                ]),
            },
            "event-error-ambiguous",
        );
    }

    // --- hf-cache sync -------------------------------------------------------

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    /// Representative repo tree exercising every §2.3 preset category.
    fn sync_tree() -> Vec<&'static str> {
        vec![
            "config.json",
            "model.safetensors",
            "model.safetensors.index.json",
            "pytorch_model.bin",
            "README.md",
            "original/consolidated.safetensors",
            "tokenizer.model",
        ]
    }

    fn run_select(
        tree: &[&str],
        files: &[&str],
        include: &[&str],
        exclude: &[&str],
        preset: Option<&str>,
    ) -> Result<(Vec<String>, SelectionMode), SyncSelectionError> {
        select_sync_files(
            tree,
            &strings(files),
            &strings(include),
            &strings(exclude),
            preset,
        )
    }

    #[test]
    fn sync_selection_positional_files_win_over_every_other_selector() {
        // §2.2 precedence: positional FILE… beats --include/--exclude and
        // the preset, even when all are given at once.
        let (selected, mode) = run_select(
            &sync_tree(),
            &["config.json", "README.md"],
            &["*.safetensors"],
            &[],
            Some("vllm"),
        )
        .unwrap();
        assert_eq!(selected, strings(&["config.json", "README.md"]));
        assert_eq!(mode, SelectionMode::Files);
    }

    #[test]
    fn sync_selection_positional_missing_and_dedup() {
        let tree = sync_tree();
        let err = run_select(&tree, &["nope.json"], &[], &[], None).unwrap_err();
        match err {
            SyncSelectionError::MissingPositional { path, available } => {
                assert_eq!(path, "nope.json");
                assert_eq!(available, strings(&tree));
            }
            other => panic!("expected MissingPositional, got {:?}", other),
        }
        // Repeated positional files collapse (order preserved).
        let (selected, _) = run_select(
            &tree,
            &["config.json", "config.json", "README.md"],
            &[],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(selected, strings(&["config.json", "README.md"]));
    }

    #[test]
    fn sync_selection_include_exclude_glob_mode() {
        let tree = sync_tree();
        let (selected, mode) = run_select(
            &tree,
            &[],
            &["*.json"],
            &["model.safetensors.index.json"],
            None,
        )
        .unwrap();
        assert_eq!(selected, strings(&["config.json"]));
        assert_eq!(mode, SelectionMode::Patterns);

        // Exclude alone (no include, no preset) filters the whole repo —
        // mode stays WholeRepo with the universal exclude on top (§2.2).
        let (selected, mode) =
            run_select(&tree, &[], &[], &["*.bin", "*.md", "original/**"], None).unwrap();
        assert_eq!(
            selected,
            strings(&[
                "config.json",
                "model.safetensors",
                "model.safetensors.index.json",
                "tokenizer.model",
            ])
        );
        assert_eq!(mode, SelectionMode::WholeRepo);
    }

    #[test]
    fn sync_selection_vllm_preset_applies_allow_and_ignore_tables() {
        // §2.3 via patterns::VLLM_ALLOW/VLLM_IGNORE: safetensors (incl.
        // subfolders, `*` crosses `/`), config/tokenizer files in; fallback
        // weight formats, docs, and original/ out.
        let (selected, mode) = run_select(&sync_tree(), &[], &[], &[], Some("vllm")).unwrap();
        assert_eq!(
            selected,
            strings(&[
                "config.json",
                "model.safetensors",
                "model.safetensors.index.json",
                "tokenizer.model",
            ])
        );
        assert_eq!(mode, SelectionMode::Preset);

        // §2.2 precedence: --include beats the preset; --exclude applies
        // ON TOP of the preset — the preset selection minus the excluded
        // file, still reported as Preset mode.
        let (selected, mode) =
            run_select(&sync_tree(), &[], &[], &["tokenizer.model"], Some("vllm")).unwrap();
        assert_eq!(mode, SelectionMode::Preset);
        assert_eq!(
            selected,
            strings(&[
                "config.json",
                "model.safetensors",
                "model.safetensors.index.json",
            ])
        );
    }

    #[test]
    fn sync_selection_whole_repo_is_the_last_resort() {
        let tree = sync_tree();
        let (selected, mode) = run_select(&tree, &[], &[], &[], None).unwrap();
        assert_eq!(selected, strings(&tree));
        assert_eq!(mode, SelectionMode::WholeRepo); // caller prints the tip
    }

    #[test]
    fn sync_selection_empty_after_filtering_is_a_usage_error() {
        let tree = sync_tree();
        // Include matching nothing.
        let err = run_select(&tree, &[], &["*.nonexistent"], &[], None).unwrap_err();
        match &err {
            SyncSelectionError::EmptySelection { available } => {
                assert_eq!(*available, strings(&tree));
            }
            other => panic!("expected EmptySelection, got {:?}", other),
        }
        // --exclude applies on top of positional files (§2.2): dropping the
        // only positional file empties the selection.
        let err = run_select(&tree, &["README.md"], &[], &["*.md"], None).unwrap_err();
        assert_eq!(err.code(), "empty_selection");
        assert_eq!(
            err.message(),
            "selection matched no files in the repository"
        );
        // Unknown preset (unreachable via clap's parse_preset, direct callers).
        let err = run_select(&tree, &[], &[], &[], Some("transformers")).unwrap_err();
        assert_eq!(err.code(), "unknown_preset");
    }

    // --- hf-cache parsing -----------------------------------------------------

    #[test]
    fn parses_hf_cache_sync_positional_files_and_flags() {
        let cli = Cli::try_parse_from([
            "rhd",
            "hf-cache",
            "sync",
            "a/b",
            "config.json",
            "tokenizer.model",
            "--revision",
            "2.0bpw",
            "--include",
            "*.safetensors",
            "--exclude",
            "original/**",
            "--cache-dir",
            "/tmp/hub",
            "--no-symlinks",
            "--force",
            "--dry-run",
            "--no-verify",
            "--json",
            "--quiet",
            "--rate-limit-mbps",
            "7.25",
        ])
        .unwrap();
        let Some(Command::HfCache(HfCacheArgs {
            command: HfCacheCommand::Sync(args),
        })) = cli.command
        else {
            panic!("expected hf-cache sync");
        };
        assert_eq!(args.model_id, "a/b");
        assert_eq!(args.files, vec!["config.json", "tokenizer.model"]);
        assert_eq!(args.revision.as_deref(), Some("2.0bpw"));
        assert_eq!(args.include, vec!["*.safetensors"]);
        assert_eq!(args.exclude, vec!["original/**"]);
        assert_eq!(args.cache_dir.as_deref(), Some("/tmp/hub"));
        assert!(args.no_symlinks);
        assert!(args.force);
        assert!(args.dry_run);
        assert!(args.no_verify);
        assert!(args.json);
        assert!(args.quiet);
        assert_eq!(args.rate_limit_mbps, Some(7.25));
        assert!(!args.rate_limit);
        assert!(!args.no_rate_limit);
    }

    #[test]
    fn parses_hf_cache_sync_for_vllm_and_sha_revision() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let cli = Cli::try_parse_from([
            "rhd",
            "hf-cache",
            "sync",
            "Qwen/Qwen2.5-7B-Instruct",
            "--for",
            "vllm",
            "--revision",
            sha,
        ])
        .unwrap();
        let Some(Command::HfCache(HfCacheArgs {
            command: HfCacheCommand::Sync(args),
        })) = cli.command
        else {
            panic!("expected hf-cache sync");
        };
        assert_eq!(args.model_id, "Qwen/Qwen2.5-7B-Instruct");
        assert!(args.files.is_empty());
        assert_eq!(args.for_preset.as_deref(), Some("vllm"));
        assert_eq!(args.revision.as_deref(), Some(sha));
    }

    #[test]
    fn hf_cache_sync_rejects_unknown_preset_bad_revision_and_flag_conflicts() {
        // --for validates its preset with a clear error.
        let err = Cli::try_parse_from(["rhd", "hf-cache", "sync", "a/b", "--for", "transformers"])
            .unwrap_err();
        assert!(err.to_string().contains("available presets: vllm"));
        // parse_revision runs at the parser level.
        assert!(
            Cli::try_parse_from(["rhd", "hf-cache", "sync", "a/b", "--revision", ".."]).is_err()
        );
        // Shared rate-limit flags keep their conflicts.
        assert!(Cli::try_parse_from([
            "rhd",
            "hf-cache",
            "sync",
            "a/b",
            "--rate-limit",
            "--no-rate-limit",
        ])
        .is_err());
        // Model id required.
        assert!(Cli::try_parse_from(["rhd", "hf-cache", "sync"]).is_err());
    }

    #[test]
    fn parses_hf_cache_path_flags() {
        let cli = Cli::try_parse_from([
            "rhd",
            "hf-cache",
            "path",
            "a/b",
            "--revision",
            "main",
            "--cache-dir",
            "/tmp/hub",
            "--token",
            "hf_x",
        ])
        .unwrap();
        let Some(Command::HfCache(HfCacheArgs {
            command: HfCacheCommand::Path(args),
        })) = cli.command
        else {
            panic!("expected hf-cache path");
        };
        assert_eq!(args.model_id, "a/b");
        assert_eq!(args.revision.as_deref(), Some("main"));
        assert_eq!(args.cache_dir.as_deref(), Some("/tmp/hub"));
        assert_eq!(args.token.as_deref(), Some("hf_x"));

        // Absent revision → None (main is applied at run time).
        let cli = Cli::try_parse_from(["rhd", "hf-cache", "path", "a/b"]).unwrap();
        let Some(Command::HfCache(HfCacheArgs {
            command: HfCacheCommand::Path(args),
        })) = cli.command
        else {
            panic!()
        };
        assert_eq!(args.revision, None);
    }

    // --- refs/symlink/path pure helpers ---------------------------------------

    #[test]
    fn refs_are_written_for_branches_tags_but_not_raw_shas() {
        // Branch/tag names get a refs/<name> entry (R2) — including
        // slash-separated branches and hex-looking short names.
        assert_eq!(ref_name_for_revision("main"), Some("main"));
        assert_eq!(ref_name_for_revision("2.0bpw"), Some("2.0bpw"));
        assert_eq!(ref_name_for_revision("release/v2"), Some("release/v2"));
        assert_eq!(
            ref_name_for_revision("0123456789abcdef"),
            Some("0123456789abcdef")
        );
        // 40-hex commit SHAs (either case) get NO refs/ entry.
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(ref_name_for_revision(sha), None);
        let upper = "ABCDEF0123456789ABCDEF0123456789ABCDEF01";
        assert_eq!(ref_name_for_revision(upper), None);
        // 40 chars but not hex: a (weird) branch name — still gets a ref.
        let not_hex = "z".repeat(40);
        assert_eq!(ref_name_for_revision(&not_hex), Some(not_hex.as_str()));
    }

    #[test]
    fn symlink_policy_honors_flag_and_hub_env_var() {
        // Default: symlinks on Unix, hub's degraded no-symlink cache on
        // Windows (relative `/`-separator targets fail to resolve there).
        #[cfg(unix)]
        assert!(symlinks_enabled(false, None));
        #[cfg(windows)]
        assert!(!symlinks_enabled(false, None));
        // --no-symlinks forces the copy fallback (R4).
        assert!(!symlinks_enabled(true, None));
        // HF_HUB_DISABLE_SYMLINKS disables too (E10, hub parity).
        assert!(!symlinks_enabled(false, Some("1")));
        assert!(!symlinks_enabled(false, Some("true")));
        assert!(!symlinks_enabled(false, Some(" YES ")));
        #[cfg(unix)]
        {
            assert!(symlinks_enabled(false, Some("0")));
            assert!(symlinks_enabled(false, Some("")));
        }
        assert!(!symlinks_enabled(true, Some("1")));
    }

    #[test]
    fn absolute_path_anchors_relative_paths_at_cwd() {
        // Platform-neutral: on Unix a leading-/ path is already absolute and
        // passes through verbatim; on Windows it is drive-relative, so only
        // assert absoluteness and the trailing components (same on both).
        let abs = absolute_path(Path::new("/cache/hub"));
        assert!(abs.is_absolute(), "rooted input must come back absolute");
        assert!(abs.ends_with("cache/hub"), "components preserved: {abs:?}");
        let rel = absolute_path(Path::new("hub/models--a--b"));
        assert!(rel.is_absolute());
        assert!(rel.ends_with("hub/models--a--b"));
    }

    // --- sync JSON events ------------------------------------------------------

    #[test]
    fn sync_events_serialize_with_type_tags() {
        // Smoke: serde tagging mirrors the existing events (§2.4).
        let value = serde_json::to_value(&Event::SyncPlanned {
            model: "org/model".to_string(),
            sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
            files: vec![FileDto {
                filename: "model.safetensors".to_string(),
                size_bytes: 42,
                sha256: Some("a".to_string()),
            }],
            skipped: 2,
            total_bytes: 42,
        })
        .unwrap();
        assert_eq!(value["type"], "sync_planned");
        assert_eq!(value["model"], "org/model");
        assert_eq!(value["skipped"], 2);

        let value = serde_json::to_value(&Event::FilePublished {
            path: "config.json".to_string(),
            blob: "deadbeef".to_string(),
        })
        .unwrap();
        assert_eq!(value["type"], "file_published");
        assert_eq!(value["path"], "config.json");
        assert_eq!(value["blob"], "deadbeef");

        let value = serde_json::to_value(&Event::SyncComplete {
            snapshot_path: "/x/snapshots/abc".to_string(),
            revision: "main".to_string(),
            sha: "abc".to_string(),
        })
        .unwrap();
        assert_eq!(value["type"], "sync_complete");
        assert_eq!(value["snapshot_path"], "/x/snapshots/abc");
        assert_eq!(value["revision"], "main");
    }

    #[test]
    fn snapshot_event_sync_planned() {
        snap(
            &Event::SyncPlanned {
                model: "org/model".to_string(),
                sha: "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
                files: vec![FileDto {
                    filename: "model-00001-of-00002.safetensors".to_string(),
                    size_bytes: 4_947_802_324,
                    sha256: Some("a".repeat(64)),
                }],
                skipped: 3,
                total_bytes: 4_947_802_324,
            },
            "event-sync-planned",
        );
    }

    #[test]
    fn snapshot_event_file_published() {
        snap(
            &Event::FilePublished {
                path: "model-00001-of-00002.safetensors".to_string(),
                blob: "a".repeat(64),
            },
            "event-file-published",
        );
    }

    #[test]
    fn snapshot_event_sync_complete() {
        snap(
            &Event::SyncComplete {
                snapshot_path: "/home/u/.cache/huggingface/hub/models--org--model/snapshots/f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
                revision: "main".to_string(),
                sha: "f6e3ba1a0b7d54e967a20e8dccd1e42e7e9b1234".to_string(),
            },
            "event-sync-complete",
        );
    }
}
