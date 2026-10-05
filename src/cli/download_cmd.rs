//! Download orchestration: config → resolve → engine bootstrap → event
//! drain. `engine::bootstrap` owns the startup sequence; the shared
//! drain loop and run tally live in [`super::run`] (Runner).

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use super::args::{apply_rate_limit_overrides, merge_token, valid_model_id, DownloadArgs};
use super::events::{ErrorCode, Event, FileDto, Summary};
use super::report::Reporter;
use super::resolve::{parse_selector, resolve_files, Selector};
use super::run::{monitor, RunTally};
use super::{EXIT_FAILURE, EXIT_INTERRUPTED, EXIT_USAGE};
use crate::engine::{EnqueuePolicy, QueuedDownload};

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
    let (state, download_tx, manager) = crate::engine::bootstrap().await;

    // Model files land under base/author/model-name (same layout as the TUI)
    let parts: Vec<&str> = args.model_id.split('/').collect();
    let model_path = PathBuf::from(&base).join(parts[0]).join(parts[1]);

    let queued: Vec<QueuedDownload> = files
        .iter()
        .map(|file| QueuedDownload {
            model_id: args.model_id.clone(),
            revision: revision.clone(),
            filename: file.filename.clone(),
            base_path: model_path.clone(),
            expected_sha256: file.sha256.clone(),
            hf_token: token.clone(),
            total_size: file.size_bytes,
        })
        .collect();
    // Shared enqueue transaction, CLI flavor: register_pending's
    // validate-first DISK upsert (the first invalid filename aborts with
    // nothing queued or sent), queue accounted before the sends, HUD
    // summaries pushed up front, no failed-send rollback. Reordering
    // note: register_pending used to run before bootstrap; moving it
    // inside enqueue (after bootstrap) is output-identical — bootstrap
    // emits nothing, and the CLI never reads the (now unseeded-with-
    // pending) registry mirror: disk is the source of truth.
    let outcome = state
        .enqueue(&download_tx, &queued, &EnqueuePolicy::cli_download(&base))
        .await;
    if let Some(err) = outcome.aborted {
        reporter.emit(&Event::error(ErrorCode::InvalidPath, err.to_string()));
        return EXIT_FAILURE;
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
        reporter.emit(&Event::error(
            ErrorCode::Interrupted,
            "interrupted by SIGINT; unfinished files stay registered as incomplete and restart from scratch on the next run".to_string(),
        ));
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
