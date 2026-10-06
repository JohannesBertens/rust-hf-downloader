//! Cross-command run machinery (Runner): the drain loop, the run tally,
//! and the bootstrap/failure-emission helpers the engine-driving CLI
//! subcommands share (`download`, `hf-cache sync`) plus the token-only
//! bootstrap the query-only ones use (`hf-cache path`, `search`).
//! Extracted from `download_cmd.rs` (W3.7+W4.2+W4.3); per-command event
//! emission stays at the call sites.
//!
//! Lock-ordering contract (from the AGENTS.md hierarchy — the reason this
//! module exists as the single home of the drain loop): [`monitor`] holds
//! no engine lock across an await (it `select!`s on the manager join, a
//! ticker, and ctrl-c); [`poll_once`] takes every lock with `try_lock` in
//! its own scope, never nested, touching in order `status_rx` (level 10),
//! `outcome_rx`, `verification_progress` (7), `verify_rx`,
//! `download_progress` (3) — a missed `try_lock` skips that tick's
//! heartbeat rather than printing a lock artifact. A guard must never
//! outlive its statement, and no lock is ever held while acquiring
//! another.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::args::{apply_rate_limit_overrides, merge_token};
use super::events::{ErrorCode, Event, FileStatus, OverallProgress};
use super::report::Reporter;
use super::resolve::FileSpec;
use super::{EXIT_FAILURE, EXIT_USAGE};
use crate::engine::{EngineState, EnqueuePolicy, ManagerHandle, QueuedDownload};
use crate::models::{AppOptions, FileOutcome, VerifyOutcome};

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
    pub(super) fn exit_code(&self) -> i32 {
        if self.auth_required {
            super::EXIT_AUTH
        } else if self.failed > 0 || self.hash_mismatch > 0 {
            EXIT_FAILURE
        } else {
            super::EXIT_OK
        }
    }
}

/// Fold the duplicated run bootstrap of `download` and `hf-cache sync`
/// (both sites' "1. Configuration" step): load config → optional
/// output-dir override (`download` only — `hf-cache sync` targets the hub
/// cache) → rate-limit flag overrides → token precedence merge
/// (`--token` > `$HF_TOKEN` > config; pinned by the token-matrix tests)
/// → token writeback → `apply_options` → `--no-verify` store. The tail
/// order is load-bearing: the no-verify store must run AFTER
/// `apply_options`, which re-enables verification from the config value.
///
/// The tail also builds the run's ONE shared `reqwest::Client` (plan
/// M4/B5) with the merged token installed as its default auth header;
/// every API call of the run threads it through. A token that cannot be
/// represented in a header value fails HERE — [`emit_client_error`]
/// surfaces it as an auth failure instead of the silent unauthenticated
/// downgrade that used to appear as a confusing 401 later.
pub(super) fn load_run_config(
    token_flag: Option<String>,
    output: Option<&str>,
    rate_limit: bool,
    no_rate_limit: bool,
    rate_limit_mbps: Option<f64>,
    no_verify: bool,
) -> Result<(AppOptions, Option<String>, reqwest::Client), crate::http_client::ClientBuildError> {
    let mut options = crate::config::load_config();
    if let Some(dir) = output {
        options.default_directory = dir.to_string();
    }
    apply_rate_limit_overrides(&mut options, rate_limit, no_rate_limit, rate_limit_mbps);
    let token = merge_token(
        token_flag,
        std::env::var("HF_TOKEN").ok(),
        options.hf_token.clone(),
    );
    options.hf_token = token.clone();
    crate::config::apply_options(&options);
    if no_verify {
        crate::download::DOWNLOAD_CONFIG
            .enable_verification
            .store(false, Ordering::Relaxed);
    }
    let client = crate::http_client::build_client_with_token(token.as_deref(), None)?;
    Ok((options, token, client))
}

/// Surface a shared-client build failure (the [`load_run_config`] tail,
/// M4/B5): a malformed token is an explicit `auth_required` +
/// [`super::EXIT_AUTH`] — the documented home of "bad token" — and a
/// plain client-build failure is `network` + [`EXIT_FAILURE`]. Either
/// way the run stops BEFORE any request goes out unauthenticated.
pub(super) fn emit_client_error(
    reporter: &mut Reporter,
    error: &crate::http_client::ClientBuildError,
) -> i32 {
    use crate::http_client::ClientBuildError;
    let code = match error {
        ClientBuildError::InvalidToken => ErrorCode::AuthRequired,
        ClientBuildError::Build(_) => ErrorCode::Network,
    };
    reporter.emit(&Event::error(
        code,
        format!(
            "{} — fix or remove the token (--token, $HF_TOKEN, or the config file)",
            error
        ),
    ));
    if matches!(error, ClientBuildError::InvalidToken) {
        super::EXIT_AUTH
    } else {
        EXIT_FAILURE
    }
}

/// The partial bootstrap the query-only subcommands share (`hf-cache
/// path`'s online fallback, `search`): resolve the token by the run
/// precedence from already-loaded options (`--token` > `$HF_TOKEN` >
/// config) — no engine, no rate-limit flags, no `apply_options`. The
/// caller keeps the loaded options for its own defaults (search params,
/// or nothing). §8.8: `AppOptions::default()` itself reads `$HF_TOKEN`,
/// so the no-config-file path already carries the env token; the env axis
/// wins over the file axis either way (pinned by the token-matrix tests —
/// the dual read is unobservable here).
pub(super) fn resolve_run_token(
    token_flag: Option<String>,
    options: &AppOptions,
) -> Option<String> {
    merge_token(
        token_flag,
        std::env::var("HF_TOKEN").ok(),
        options.hf_token.clone(),
    )
}

/// `--revision` value or the hub default (`main`) — the shared defaulting
/// step of `download`, `hf-cache sync`, and `hf-cache path`.
pub(super) fn effective_revision(revision: &Option<String>) -> String {
    revision
        .clone()
        .unwrap_or_else(|| crate::api::DEFAULT_REVISION.to_string())
}

/// Metadata-fetch failure shared by `download` and `hf-cache sync`
/// (byte-identical blocks pre-extraction): HTTP 404 maps to a
/// `not_found` error event + `EXIT_USAGE`, anything else to `network` +
/// `EXIT_FAILURE`. `not_found` is deliberately outside the [`ErrorCode`]
/// wire set (dynamic code, W1.3 decision) — the raw construction below is
/// what both sites did.
pub(super) fn emit_metadata_error(
    reporter: &mut Reporter,
    model_id: &str,
    error: &reqwest::Error,
) -> i32 {
    let not_found = error.status() == Some(reqwest::StatusCode::NOT_FOUND);
    reporter.emit(&Event::Error {
        code: if not_found {
            "not_found".to_string()
        } else {
            "network".to_string()
        },
        message: format!("failed to fetch model info for {}: {}", model_id, error),
        available: None,
    });
    if not_found {
        EXIT_USAGE
    } else {
        EXIT_FAILURE
    }
}

/// The shared run-tail error events, in the one order both engine sites
/// emit them: download failures (joined), publish failures (`hf-cache
/// sync` only — pass `&[]` from `download`), hash mismatches (joined),
/// auth-required. The interrupted event and the exit-code arithmetic stay
/// at the call sites (their messages and interleavings differ per
/// command).
pub(super) fn emit_run_failures(
    reporter: &mut Reporter,
    tally: &RunTally,
    model_id: &str,
    publish_failures: &[String],
) {
    if !tally.failures.is_empty() {
        reporter.emit(&Event::error(
            ErrorCode::DownloadFailed,
            tally.failures.join("; "),
        ));
    }
    if !publish_failures.is_empty() {
        reporter.emit(&Event::error(
            ErrorCode::PublishFailed,
            publish_failures.join("; "),
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
                model_id
            ),
        ));
    }
}

/// Bootstrap the engine, hand the queue over, and close the channel:
/// `engine::bootstrap` → [`EngineState::enqueue`] → `drop(download_tx)`.
///
/// The drop point is byte-position-identical to both legacy sites
/// (immediately after the enqueue transaction, before the monitor loop):
/// dropping the LAST sender closes the channel — the manager drains its
/// queue and its join handle resolves, which is the deterministic
/// completion signal the monitor awaits. On the `download` abort path the
/// caller returns without monitoring; there the sender now closes inside
/// this function instead of at caller-scope end — unobservable: nothing
/// was queued or sent on abort, the manager emits nothing, and the join
/// handle is simply dropped (detached). Enforced end-to-end by the exit
/// code matrix (H3), human goldens (H4), and the NDJSON e2e suite.
pub(super) async fn queue_run(
    queued: &[QueuedDownload],
    policy: &EnqueuePolicy,
) -> (EngineState, ManagerHandle, crate::engine::EnqueueOutcome) {
    let (state, download_tx, manager) = crate::engine::bootstrap().await;
    let outcome = state.enqueue(&download_tx, queued, policy).await;
    // Dropping the sender closes the channel — the manager drains, then
    // its join handle resolves. This is the deterministic completion signal.
    drop(download_tx);
    (state, manager, outcome)
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

/// One home for the per-outcome tally mutation: the streaming arms in
/// [`apply_outcome_event`] and the authoritative recount in
/// [`count_outcomes`] must agree forever — this function IS that
/// agreement. Everything else (event emission, the outcomes list) stays
/// with the callers.
fn tally_outcome(outcome: &FileOutcome, tally: &mut RunTally) {
    match outcome {
        FileOutcome::Complete { bytes, .. } => {
            tally.downloaded += 1;
            tally.done_bytes += *bytes;
        }
        FileOutcome::AlreadyExists { bytes, .. } => {
            tally.skipped += 1;
            tally.done_bytes += *bytes;
        }
        FileOutcome::AuthRequired { .. } => tally.auth_required = true,
        FileOutcome::Failed { filename, reason } => {
            tally.failed += 1;
            tally.failures.push(format!("{}: {}", filename, reason));
        }
    }
}

fn apply_outcome_event(
    outcome: &FileOutcome,
    index_of: &HashMap<&str, usize>,
    count: usize,
    reporter: &mut Reporter,
    tally: &mut RunTally,
) {
    tally_outcome(outcome, tally);
    match outcome {
        FileOutcome::Complete { filename, bytes } => {
            reporter.emit(&Event::FileComplete {
                filename: filename.clone(),
                status: FileStatus::Downloaded,
                bytes: *bytes,
            });
        }
        FileOutcome::AlreadyExists { filename, bytes } => {
            reporter.emit(&Event::FileComplete {
                filename: filename.clone(),
                status: FileStatus::AlreadyExists,
                bytes: *bytes,
            });
        }
        FileOutcome::AuthRequired { model_id } => {
            reporter.emit(&Event::error(
                ErrorCode::AuthRequired,
                format!(
                    "authentication required for {} (pass --token or set $HF_TOKEN)",
                    model_id
                ),
            ));
        }
        FileOutcome::Failed { filename, reason } => {
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
        tally_outcome(outcome, tally);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Shareable in-memory stderr sink (mirrors the one in `cli::tests`).
    #[derive(Clone, Default)]
    struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Sink {
        fn take(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().drain(..).collect()).unwrap()
        }
    }

    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// MANDATORY Runner gate: the streaming tally (poll_once drains
    /// `outcome_rx` through `apply_outcome_event`) and the authoritative
    /// recount (`count_outcomes` over the manager join list) must agree on
    /// every tally field they both maintain, over a synthetic mixed
    /// outcome list covering every variant (with repeats).
    #[test]
    fn streaming_tally_matches_authoritative_recount_on_mixed_outcomes() {
        let outcomes = vec![
            FileOutcome::Complete {
                filename: "a.gguf".to_string(),
                bytes: 100,
            },
            FileOutcome::AlreadyExists {
                filename: "b.gguf".to_string(),
                bytes: 50,
            },
            FileOutcome::Failed {
                filename: "c.gguf".to_string(),
                reason: "boom".to_string(),
            },
            FileOutcome::AuthRequired {
                model_id: "x/y".to_string(),
            },
            FileOutcome::Complete {
                filename: "d.gguf".to_string(),
                bytes: 7,
            },
            FileOutcome::Failed {
                filename: "e.gguf".to_string(),
                reason: "later".to_string(),
            },
        ];

        // Streaming path
        let sink = Sink::default();
        let mut reporter = super::super::report::Reporter::new_with_stderr(
            false,
            false,
            super::super::report::ProgressMode::None,
            Box::new(sink.clone()),
        );
        let mut streamed = RunTally::default();
        let index_of = HashMap::new();
        for outcome in &outcomes {
            apply_outcome_event(
                outcome,
                &index_of,
                outcomes.len(),
                &mut reporter,
                &mut streamed,
            );
        }

        // Authoritative recount
        let mut recounted = RunTally::default();
        count_outcomes(&outcomes, &mut recounted);

        // Absolute expectations first (the semantics), then agreement.
        assert_eq!(streamed.downloaded, 2);
        assert_eq!(streamed.skipped, 1);
        assert_eq!(streamed.failed, 2);
        assert_eq!(streamed.done_bytes, 157);
        assert!(streamed.auth_required);
        assert_eq!(
            streamed.failures,
            vec!["c.gguf: boom".to_string(), "e.gguf: later".to_string()]
        );

        assert_eq!(streamed.downloaded, recounted.downloaded);
        assert_eq!(streamed.skipped, recounted.skipped);
        assert_eq!(streamed.failed, recounted.failed);
        assert_eq!(streamed.done_bytes, recounted.done_bytes);
        assert_eq!(streamed.auth_required, recounted.auth_required);
        assert_eq!(streamed.failures, recounted.failures);
        // (hash_mismatch / verified / mismatches are NOT compared here:
        // this fixture streams download outcomes only, so both sides are
        // trivially 0/empty — a vacuous 0 == 0. The verification counters
        // are pinned against literals by `apply_verify_outcome_counters_`
        // `and_human_lines_are_literal` below; they are not part of the
        // streaming-vs-recount agreement because only `apply_verify_outcome`
        // ever touches them.)
        // The recount additionally installs the authoritative list (the
        // publish gate's input) — streaming never populates it.
        assert_eq!(recounted.outcomes.len(), outcomes.len());
        assert!(streamed.outcomes.is_empty());

        // One human line per streamed outcome (2 completes + 2 failures +
        // 1 auth error + 1 already-exists = 6).
        assert_eq!(sink.take().lines().count(), 6);
    }

    /// T2 (test-hardening): drive [`apply_verify_outcome`] over every
    /// `VerifyOutcome` variant and pin BOTH the tally counters and the
    /// exact human stderr bytes — literal expectations, no self-comparison.
    #[test]
    fn apply_verify_outcome_counters_and_human_lines_are_literal() {
        let sink = Sink::default();
        let mut reporter = super::super::report::Reporter::new_with_stderr(
            false,
            false,
            super::super::report::ProgressMode::None,
            Box::new(sink.clone()),
        );
        let mut tally = RunTally::default();

        apply_verify_outcome(
            &VerifyOutcome::Ok {
                filename: "a.gguf".to_string(),
            },
            &mut reporter,
            &mut tally,
        );
        apply_verify_outcome(
            &VerifyOutcome::Mismatch {
                filename: "b.gguf".to_string(),
                expected_sha256: "dead".to_string(),
                actual_sha256: "beef".to_string(),
            },
            &mut reporter,
            &mut tally,
        );
        apply_verify_outcome(
            &VerifyOutcome::Error {
                filename: "c.gguf".to_string(),
                reason: "read failed".to_string(),
            },
            &mut reporter,
            &mut tally,
        );
        apply_verify_outcome(
            &VerifyOutcome::Missing {
                filename: "d.gguf".to_string(),
            },
            &mut reporter,
            &mut tally,
        );

        // Literal counters: Ok bumps `verified`, Mismatch bumps
        // `hash_mismatch` and records the filename, Error/Missing touch
        // NEITHER counter (error-event only — the exit-code contribution
        // of a failed verification run comes from the mismatch arm and the
        // CLI's own summary, pinned in cli_exit_codes).
        assert_eq!(tally.verified, 1);
        assert_eq!(tally.hash_mismatch, 1);
        assert_eq!(tally.mismatches, vec!["b.gguf".to_string()]);
        assert_eq!(tally.verify_outcomes.len(), 4);

        // Literal human lines, in emission order.
        assert_eq!(
            sink.take(),
            concat!(
                " ✓ verified: a.gguf\n",
                " ✗ verified hash mismatch: b.gguf\n",
                "error [verification_error]: c.gguf: read failed\n",
                "error [verification_error]: d.gguf: file not found for verification\n",
            )
        );
    }

    #[test]
    fn effective_revision_defaults_to_main_only_when_unset() {
        assert_eq!(
            effective_revision(&None),
            crate::api::DEFAULT_REVISION.to_string()
        );
        assert_eq!(effective_revision(&Some("v1.2".to_string())), "v1.2");
        // A 40-hex SHA revision passes through verbatim (validation is
        // clap's parse_revision job, not this helper's).
        assert_eq!(
            effective_revision(&Some("deadbeef".repeat(5))),
            "deadbeef".repeat(5)
        );
    }

    /// Pins the shared run-tail emission ORDER (failures → publish
    /// failures → mismatches → auth) that both engine sites rely on, and
    /// the exact human bytes of each line.
    #[test]
    fn emit_run_failures_order_and_bytes() {
        let sink = Sink::default();
        let mut reporter = super::super::report::Reporter::new_with_stderr(
            false,
            false,
            super::super::report::ProgressMode::None,
            Box::new(sink.clone()),
        );
        let mut tally = RunTally {
            failures: vec!["a.gguf: boom".to_string(), "b.gguf: late".to_string()],
            mismatches: vec!["c.gguf".to_string()],
            auth_required: true,
            ..RunTally::default()
        };

        // download flavor: empty publish list → no publish_failed event.
        emit_run_failures(&mut reporter, &tally, "x/y", &[]);
        assert_eq!(
            sink.take(),
            concat!(
                "error [download_failed]: a.gguf: boom; b.gguf: late\n",
                "error [hash_mismatch]: c.gguf\n",
                "error [auth_required]: authentication required for x/y (pass --token or set $HF_TOKEN)\n",
            )
        );

        // hf-cache sync flavor: publish failures slot in between.
        tally.failures.clear(); // exercises the empty-failures skip too
        emit_run_failures(
            &mut reporter,
            &tally,
            "x/y",
            &["d.gguf: no verification result before publish".to_string()],
        );
        assert_eq!(
            sink.take(),
            concat!(
                "error [publish_failed]: d.gguf: no verification result before publish\n",
                "error [hash_mismatch]: c.gguf\n",
                "error [auth_required]: authentication required for x/y (pass --token or set $HF_TOKEN)\n",
            )
        );
    }
}
