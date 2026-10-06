//! Download orchestration: config → resolve → engine bootstrap → event
//! drain. `engine::bootstrap` owns the startup sequence; the shared
//! config fold, queue handoff, drain loop, and run-tail emissions live in
//! [`super::run`] (Runner).

use std::path::PathBuf;

use super::args::{valid_model_id, DownloadArgs};
use super::events::{ErrorCode, Event, FileDto, Summary};
use super::report::Reporter;
use super::resolve::{parse_selector, resolve_files, selection_error_event, Selector};
use super::run::{
    effective_revision, emit_metadata_error, emit_run_failures, load_run_config, monitor,
    queue_run, RunTally,
};
use super::{EXIT_FAILURE, EXIT_INTERRUPTED, EXIT_USAGE};
use crate::engine::{EnqueuePolicy, QueuedDownload};

pub(super) async fn run_download(args: DownloadArgs) -> i32 {
    let mut reporter = Reporter::new(
        args.run_output.json,
        args.run_output.quiet,
        args.run_output.progress,
    );

    // --- 1. Configuration (Runner fold: run::load_run_config) ------------
    let (options, token) = load_run_config(
        args.run_output.token.clone(),
        args.output.as_deref(),
        args.rate_limits.rate_limit,
        args.rate_limits.no_rate_limit,
        args.rate_limits.rate_limit_mbps,
        args.run_output.no_verify,
    );

    // --- 2. Validate usage ------------------------------------------------
    let revision = effective_revision(&args.revision);
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
            Err(e) => return emit_metadata_error(&mut reporter, &args.model_id, &e),
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
            reporter.emit(&selection_error_event(
                &err,
                err.available().iter().map(FileDto::from).collect(),
            ));
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
    // notes (both proven output-identical): register_pending used to run
    // before bootstrap — bootstrap emits nothing and the CLI reads the
    // registry from disk only; queue_run builds nothing between
    // bootstrap, the enqueue, and the sender drop (see run::queue_run).
    let (state, manager, outcome) = queue_run(&queued, &EnqueuePolicy::cli_download(&base)).await;
    if let Some(err) = outcome.aborted {
        reporter.emit(&Event::error(ErrorCode::InvalidPath, err.to_string()));
        return EXIT_FAILURE;
    }

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
    emit_run_failures(&mut reporter, &tally, &args.model_id, &[]);

    tally.exit_code()
}
