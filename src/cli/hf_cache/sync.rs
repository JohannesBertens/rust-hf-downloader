//! `hf-cache sync` (plans/hf-cache-sync.md §5.2 pipeline, followed exactly): config bootstrap →
//! tree + commit SHA → selection → cache plan → engine run → publish gate
//! → refs/staging cleanup → summary and snapshot path.
//!
//! Split out of `cli/hf_cache_cmd.rs` (plan W3.5); bodies moved verbatim.
//! The pure selector lives in [`super::selection`], the scripting helper
//! `hf-cache path` in [`super::path`], and [`super::absolute_path`] is the
//! shared path helper. Runner machinery (config/token bootstrap, engine
//! bootstrap, monitor, run-tail emissions) is shared with `download`
//! through [`crate::cli::run`].

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::Ordering;

use super::absolute_path;
// Reached through the `hf_cache` facade (`pub use selection::*`), which is
// what keeps the re-export live for `cli/tests.rs`.
use super::{select_sync_files, tree_file_dtos, SelectionMode};
use crate::cli::args::{valid_model_id, HfCacheSyncArgs};
use crate::cli::events::{ErrorCode, Event, FileDto, Summary};
use crate::cli::report::Reporter;
use crate::cli::resolve::{selection_error_event, FileSpec};
use crate::cli::run::{
    effective_revision, emit_client_error, emit_metadata_error, emit_run_failures, load_run_config,
    monitor, queue_run, RunTally,
};
use crate::cli::{EXIT_AUTH, EXIT_FAILURE, EXIT_INTERRUPTED, EXIT_OK, EXIT_USAGE};
use crate::engine::{EnqueuePolicy, QueuedDownload};
use crate::models::{FileOutcome, VerifyOutcome};

/// Whole-repo sync hint (plans/hf-cache-sync.md §2.2 precedence step 4).
const TIP_USE_FOR_VLLM: &str = "tip: use --for vllm to fetch only what vLLM reads";

// --- sync-pipeline helpers (pure or filesystem-local) ----------------------

/// The `refs/` name for a revision (R2): branch/tag revisions are written
/// to `refs/<name>`; a raw 40-hex commit-SHA revision gets **no** ref (hub
/// behavior — the snapshot is addressed by SHA alone).
pub fn ref_name_for_revision(revision: &str) -> Option<&str> {
    let is_commit_sha = revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit());
    if is_commit_sha {
        None
    } else {
        Some(revision)
    }
}

/// Hub-parity truthiness for `HF_HUB_DISABLE_SYMLINKS` (E10):
/// `1`/`on`/`yes`/`true`, case-insensitive, like huggingface_hub's
/// constants module.
fn env_flag_is_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_uppercase().as_str(),
        "1" | "ON" | "YES" | "TRUE"
    )
}

/// Whether snapshot entries should be symlinks (R4): `--no-symlinks`
/// forces the copy fallback, and `HF_HUB_DISABLE_SYMLINKS` disables them
/// too (E10, hub parity). On Windows symlinks are **off by default** —
/// hub's own degraded-cache default — because relative targets with `/`
/// separators fail to resolve (os error 123) and creation needs developer
/// mode; snapshots get real files instead.
pub fn symlinks_enabled(no_symlinks_flag: bool, env_value: Option<&str>) -> bool {
    if no_symlinks_flag || env_value.is_some_and(env_flag_is_true) {
        return false;
    }
    // huggingface_hub's own default on Windows is the degraded no-symlink
    // cache: relative symlink targets with `/` separators resolve as
    // ERROR_INVALID_NAME on Windows, and creation needs developer mode.
    // Mirror that default (snapshots get real files); Unix keeps symlinks.
    #[cfg(windows)]
    {
        false
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Filename carried by a download outcome (`AuthRequired` is repo-wide
/// and carries none).
fn outcome_filename(outcome: &FileOutcome) -> Option<&str> {
    match outcome {
        FileOutcome::Complete { filename, .. }
        | FileOutcome::AlreadyExists { filename, .. }
        | FileOutcome::Failed { filename, .. } => Some(filename),
        FileOutcome::AuthRequired { .. } => None,
    }
}

/// Filename every verification-result variant carries.
fn verify_outcome_filename(outcome: &VerifyOutcome) -> &str {
    match outcome {
        VerifyOutcome::Ok { filename }
        | VerifyOutcome::Mismatch { filename, .. }
        | VerifyOutcome::Error { filename, .. }
        | VerifyOutcome::Missing { filename } => filename,
    }
}

/// R6/E9 relink: ensure snapshot entries exist for already-cached files
/// (blobs present per the plan; their entries may be missing or dangling
/// after an interrupted publish). Returns the first failure, if any.
fn relink_up_to_date(
    repo_dir: &Path,
    sha: &str,
    tree: &[crate::models::RepoFile],
    up_to_date: &[String],
    use_symlinks: bool,
) -> Result<(), String> {
    for path in up_to_date {
        let Some(file) = tree.iter().find(|f| f.rfilename == *path) else {
            continue; // plan() only reports tree paths; skip defensively
        };
        if let Err(e) =
            crate::cache_layout::ensure_snapshot_entry(repo_dir, sha, file, use_symlinks)
        {
            return Err(format!("cannot link snapshot entry for {path}: {e}"));
        }
    }
    Ok(())
}

/// Human dry-run table (plans/hf-cache-sync.md §5.2 step 4): per-file fetch/cached rows with
/// sizes (hf CLI parity). `--json` mode prints only the `SyncPlanned`
/// event, so this is human-only output.
fn print_sync_dry_run(
    model: &str,
    revision: &str,
    sha: &str,
    plan: &crate::cache_layout::SyncPlan,
    tree: &[crate::models::RepoFile],
    cache_dir: &Path,
) {
    let size_of = |path: &str| {
        tree.iter()
            .find(|f| f.rfilename == path)
            .and_then(|f| f.size.or_else(|| f.lfs.as_ref().map(|lfs| lfs.size)))
            .unwrap_or(0)
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "would sync {} (revision {}, commit {}) into {}",
        model,
        revision,
        sha,
        cache_dir.display()
    );
    let mut fetch_bytes = 0u64;
    for item in &plan.fetch {
        fetch_bytes += item.size;
        let _ = writeln!(
            out,
            " {:<6} {:<58} {:>9}",
            "fetch",
            crate::fmt::truncate_path_cli(&item.repo_path, 58),
            crate::fmt::size_full(item.size)
        );
    }
    for path in &plan.up_to_date {
        let _ = writeln!(
            out,
            " {:<6} {:<58} {:>9}",
            "cached",
            crate::fmt::truncate_path_cli(path, 58),
            crate::fmt::size_full(size_of(path))
        );
    }
    let _ = writeln!(
        out,
        "{} file(s) to fetch ({}), {} already cached — dry run, nothing written",
        plan.fetch.len(),
        crate::fmt::size_full(fetch_bytes),
        plan.up_to_date.len()
    );
    let _ = out.flush();
}

pub(super) async fn run_hf_cache_sync(args: HfCacheSyncArgs) -> i32 {
    let mut reporter = Reporter::new(
        args.run_output.json,
        args.run_output.quiet,
        args.run_output.progress,
    );

    // --- 1. Configuration (Runner fold: run::load_run_config, no output
    //        override — the destination is the hub cache, plans/hf-cache-sync.md §4.1) ------------
    let (_options, token, api_client) = match load_run_config(
        args.run_output.token.clone(),
        None,
        args.rate_limits.rate_limit,
        args.rate_limits.no_rate_limit,
        args.rate_limits.rate_limit_mbps,
        args.run_output.no_verify,
    ) {
        Ok(bootstrap) => bootstrap,
        Err(e) => return emit_client_error(&mut reporter, &e),
    };

    // --- 2. Validate usage (plans/hf-cache-sync.md §5.2 step 1: revision already parsed by
    //        clap's parse_revision) ----------------------------------------
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
    let revision = effective_revision(&args.revision);

    // --- 3. Tree + commit SHA in parallel (plans/hf-cache-sync.md §5.2 step 2) --------------------
    let (metadata_res, sha_res) = tokio::join!(
        crate::api::fetch_model_metadata(&api_client, &args.model_id, &revision),
        crate::api::resolve_revision_sha(&api_client, &args.model_id, &revision),
    );
    let metadata = match metadata_res {
        Ok(metadata) => metadata,
        Err(e) => return emit_metadata_error(&mut reporter, &args.model_id, &e),
    };
    let sha = match sha_res {
        Ok(sha) => sha,
        Err(e) => {
            let not_found = e.status() == Some(reqwest::StatusCode::NOT_FOUND);
            reporter.emit(&Event::Error {
                code: if not_found {
                    "unknown_revision"
                } else {
                    "network"
                }
                .to_string(),
                message: format!(
                    "failed to resolve revision {} of {}: {}",
                    revision, args.model_id, e
                ),
                available: None,
            });
            // Unknown revision (404) is a usage error (plans/hf-cache-sync.md §2.4).
            return if not_found { EXIT_USAGE } else { EXIT_FAILURE };
        }
    };

    // --- 4. Selection precedence (plans/hf-cache-sync.md §2.2) -------------------------------------
    let tree_paths: Vec<&str> = metadata
        .siblings
        .iter()
        .filter(|f| !f.rfilename.ends_with('/'))
        .map(|f| f.rfilename.as_str())
        .collect();
    let (selected, mode) = match select_sync_files(
        &tree_paths,
        &args.files,
        &args.include,
        &args.exclude,
        args.for_preset.as_deref(),
    ) {
        Ok(selection) => selection,
        Err(err) => {
            reporter.emit(&selection_error_event(&err, tree_file_dtos(&metadata)));
            return EXIT_USAGE;
        }
    };
    if mode == SelectionMode::WholeRepo {
        reporter.status_line(TIP_USE_FOR_VLLM);
    }

    // --- 5. Plan against the current cache (R6) + SyncPlanned (plans/hf-cache-sync.md §2.4) --------
    let cache_dir = crate::paths::hf_hub_cache(args.cache_dir.as_deref());
    let plan = match crate::cache_layout::plan(
        &cache_dir,
        &args.model_id,
        &metadata.siblings,
        &selected,
        &sha,
        args.force,
    ) {
        Ok(plan) => plan,
        Err(e) => {
            reporter.emit(&Event::error(
                ErrorCode::PlanFailed,
                format!("cannot plan cache sync for {}: {}", args.model_id, e),
            ));
            return EXIT_FAILURE;
        }
    };
    let total_bytes: u64 = plan.fetch.iter().map(|item| item.size).sum();
    reporter.emit(&Event::SyncPlanned {
        model: args.model_id.clone(),
        sha: sha.clone(),
        files: plan
            .fetch
            .iter()
            .map(|item| FileDto {
                filename: item.repo_path.clone(),
                size_bytes: item.size,
                sha256: item.sha256.clone(),
            })
            .collect(),
        skipped: plan.up_to_date.len(),
        total_bytes,
    });

    // --- 6. Dry run: the plan is the output; no writes (plans/hf-cache-sync.md §5.2 step 4) --------
    if args.dry_run {
        if !args.run_output.json {
            print_sync_dry_run(
                &args.model_id,
                &revision,
                &sha,
                &plan,
                &metadata.siblings,
                &cache_dir,
            );
        }
        reporter.finish();
        return EXIT_OK;
    }

    // --- 7. Fully-cached no-op (plans/hf-cache-sync.md §5.2 step 3): write refs, relink entries,
    //        print the snapshot path, exit 0.
    let repo_dir = cache_dir.join(crate::cache_layout::repo_dir_name(&args.model_id));
    let staging = crate::cache_layout::staging_dir(&repo_dir);
    let use_symlinks = symlinks_enabled(
        args.no_symlinks,
        std::env::var("HF_HUB_DISABLE_SYMLINKS").ok().as_deref(),
    );
    if plan.fetch.is_empty() {
        let _sync_lock = match acquire_sync_lock_or_fail(&staging, &mut reporter) {
            Ok(guard) => guard,
            Err(code) => return code,
        };
        if let Err(message) = relink_up_to_date(
            &repo_dir,
            &sha,
            &metadata.siblings,
            &plan.up_to_date,
            use_symlinks,
        ) {
            reporter.emit(&Event::error(ErrorCode::Io, message));
            return EXIT_FAILURE;
        }
        if let Err(e) =
            crate::cache_layout::write_refs(&repo_dir, ref_name_for_revision(&revision), &sha)
        {
            reporter.emit(&Event::error(
                ErrorCode::Io,
                format!("cannot write refs: {e}"),
            ));
            return EXIT_FAILURE;
        }
        reporter.emit(&Event::Done {
            summary: Summary {
                files: selected.len(),
                downloaded: 0,
                skipped: plan.up_to_date.len(),
                verified: 0,
                failed: 0,
                hash_mismatch: 0,
                total_bytes: 0,
            },
        });
        let snapshot_path = absolute_path(&crate::cache_layout::snapshot_dir(
            &cache_dir,
            &args.model_id,
            &sha,
        ));
        // SyncComplete's human rendering is the snapshot path itself —
        // the last line, hf CLI parity (plans/hf-cache-sync.md §2.4).
        reporter.emit(&Event::SyncComplete {
            snapshot_path: snapshot_path.display().to_string(),
            revision: revision.clone(),
            sha: sha.clone(),
        });
        reporter.finish();
        return EXIT_OK;
    }

    // --- 8. Layout dirs + CACHEDIR.TAG (plans/hf-cache-sync.md §5.2 step 5) -------------------------
    let snapshot_root = crate::cache_layout::snapshot_dir(&cache_dir, &args.model_id, &sha);
    for dir in [repo_dir.join("blobs"), snapshot_root, staging.clone()] {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            reporter.emit(&Event::error(
                ErrorCode::Io,
                format!("cannot create {}: {}", dir.display(), e),
            ));
            return EXIT_FAILURE;
        }
    }
    if let Err(e) = crate::paths::write_cachedir_tag(&cache_dir) {
        reporter.emit(&Event::error(
            ErrorCode::Io,
            format!(
                "cannot write CACHEDIR.TAG in {}: {}",
                cache_dir.display(),
                e
            ),
        ));
        return EXIT_FAILURE;
    }

    // --- 9. Sync lock (plans/hf-cache-sync.md §5.2 step 6) ------------------------------------------
    let _sync_lock = match acquire_sync_lock_or_fail(&staging, &mut reporter) {
        Ok(guard) => guard,
        Err(code) => return code,
    };
    if let Err(message) = relink_up_to_date(
        &repo_dir,
        &sha,
        &metadata.siblings,
        &plan.up_to_date,
        use_symlinks,
    ) {
        reporter.emit(&Event::error(ErrorCode::Io, message));
        return EXIT_FAILURE;
    }

    // --- 10. Engine bootstrap + enqueue (plans/hf-cache-sync.md §5.2 step 7; Runner queue_run) -----
    // Note: no pending registry entries are deliberately registered — the
    // flat-download registry is TUI-resume state (plans/hf-cache-sync.md §4.6). The engine still
    // writes registry entries for files it fetches (staging paths); those
    // are swept after the run and at the start of the next one so the
    // TUI's resume/complete views stay clean. The no-register choice is
    // named by EnqueuePolicy::hf_cache_sync, not just this comment.
    purge_staging_registry_entries();

    let files: Vec<FileSpec> = plan
        .fetch
        .iter()
        .map(|item| FileSpec {
            filename: item.repo_path.clone(),
            size_bytes: item.size,
            sha256: item.sha256.clone(),
        })
        .collect();
    // base_path = staging dir, filename = repo path (plans/hf-cache-sync.md §5.1); the
    // revision is the resolved commit SHA, so a moving branch cannot
    // race the plan. Expected sha = LFS oid; total size from the tree.
    let queued: Vec<QueuedDownload> = plan
        .fetch
        .iter()
        .map(|item| QueuedDownload {
            model_id: args.model_id.clone(),
            revision: sha.clone(),
            filename: item.repo_path.clone(),
            base_path: staging.clone(),
            expected_sha256: item.sha256.clone(),
            hf_token: token.clone(),
            total_size: item.size,
        })
        .collect();
    // Shared enqueue transaction, hf-cache flavor: queue accounted
    // before the sends, HUD summaries pushed up front, no failed-send
    // rollback — and NOTHING registered pending (the staging-sweep
    // policy; this policy cannot abort).
    let (state, manager, _outcome) = queue_run(&queued, &EnqueuePolicy::hf_cache_sync()).await;

    // --- 11. Monitor until drained (plans/hf-cache-sync.md §5.2 step 8, reusing run_download's
    //         monitor/verification-idle machinery) ---------------------------
    let mut tally = RunTally {
        files: selected.len(),
        total_bytes,
        ..RunTally::default()
    };
    let interrupted = monitor(&state, manager, &files, &mut tally, &mut reporter).await;

    // --- 12. Publish gate (plans/hf-cache-sync.md §5.2 step 9) --------------------------------------
    let verification_active = crate::download::DOWNLOAD_CONFIG
        .enable_verification
        .load(Ordering::Relaxed);
    let verify_of: HashMap<&str, &VerifyOutcome> = tally
        .verify_outcomes
        .iter()
        .map(|outcome| (verify_outcome_filename(outcome), outcome))
        .collect();
    let mut published: Vec<String> = Vec::new();
    let mut publish_failures: Vec<String> = Vec::new();
    // Sweep point is post-drain / pre-publish: the engine records this
    // run's fetches with staging paths during the drain, and the bootstrap
    // purge above only covers staging entries left by previous runs.
    if !plan.fetch.is_empty() {
        purge_staging_registry_entries();
    }
    for item in &plan.fetch {
        let Some(outcome) = tally
            .outcomes
            .iter()
            .find(|o| outcome_filename(o) == Some(item.repo_path.as_str()))
        else {
            publish_failures.push(format!("{}: no download outcome", item.repo_path));
            continue;
        };
        // Failed/AuthRequired files never reach the gate; the monitor
        // already surfaced them as Error events.
        if !matches!(
            outcome,
            FileOutcome::Complete { .. } | FileOutcome::AlreadyExists { .. }
        ) {
            continue;
        }
        let staged = staging.join(&item.repo_path);
        // Verification gate (R5): publish only with no hub digest,
        // verification deliberately skipped, or an explicit Ok.
        match &item.sha256 {
            None => {}
            Some(_) if args.run_output.no_verify => {
                reporter.status_line(&format!(
                    "Warning: publishing {} without SHA256 verification (--no-verify)",
                    item.repo_path
                ));
            }
            Some(_) if !verification_active => {} // standing config choice
            Some(_) => match verify_of.get(item.repo_path.as_str()) {
                Some(VerifyOutcome::Ok { .. }) => {}
                Some(VerifyOutcome::Mismatch { .. }) => {
                    // The bad bytes never enter the cache (plans/hf-cache-sync.md §2.4).
                    let _ = std::fs::remove_file(&staged);
                    publish_failures.push(format!(
                        "{}: SHA256 mismatch; staged copy deleted",
                        item.repo_path
                    ));
                    continue;
                }
                Some(VerifyOutcome::Error { reason, .. }) => {
                    publish_failures.push(format!(
                        "{}: verification error: {}",
                        item.repo_path, reason
                    ));
                    continue;
                }
                Some(VerifyOutcome::Missing { .. }) | None => {
                    publish_failures.push(format!(
                        "{}: no verification result before publish",
                        item.repo_path
                    ));
                    continue;
                }
            },
        }
        match crate::cache_layout::publish_one(&repo_dir, &sha, item, &staged, use_symlinks) {
            Ok(blob_oid) => {
                published.push(item.repo_path.clone());
                reporter.emit(&Event::FilePublished {
                    path: item.repo_path.clone(),
                    blob: blob_oid,
                });
            }
            Err(e) => publish_failures.push(format!("{}: {}", item.repo_path, e)),
        }
    }

    // --- 13. refs + staging cleanup (plans/hf-cache-sync.md §5.2 step 10) ----------------------------
    let mut failed = tally.failed > 0
        || tally.hash_mismatch > 0
        || tally.auth_required
        || !publish_failures.is_empty();
    if !interrupted && !failed {
        // R2: refs only for branch/tag revisions, never raw SHAs.
        if let Err(e) =
            crate::cache_layout::write_refs(&repo_dir, ref_name_for_revision(&revision), &sha)
        {
            publish_failures.push(format!("refs/{}: {}", revision, e));
            failed = true;
        } else {
            // Full success: drop staging remnants of published files;
            // .incomplete files of failed runs keep their resume value.
            let _ = crate::cache_layout::cleanup_staging(&repo_dir, &published);
        }
    }

    // --- 14. Summary + snapshot path (plans/hf-cache-sync.md §5.2 step 11, §2.4) ---------------------
    let summary = Summary {
        files: selected.len(),
        downloaded: tally.downloaded,
        skipped: plan.up_to_date.len() + tally.skipped,
        verified: tally.verified,
        failed: tally.failed,
        hash_mismatch: tally.hash_mismatch,
        total_bytes,
    };
    reporter.emit(&Event::Done { summary });
    reporter.finish();

    let snapshot_path = absolute_path(&crate::cache_layout::snapshot_dir(
        &cache_dir,
        &args.model_id,
        &sha,
    ));
    if interrupted {
        reporter.emit(&Event::error(
            ErrorCode::Interrupted,
            "interrupted by SIGINT; staged partial files resume on the next run".to_string(),
        ));
        return EXIT_INTERRUPTED;
    }
    if !failed {
        reporter.emit(&Event::SyncComplete {
            snapshot_path: snapshot_path.display().to_string(),
            revision: revision.clone(),
            sha: sha.clone(),
        });
        // SyncComplete's human rendering IS the last line (plans/hf-cache-sync.md §2.4) — the
        // snapshot path, printed even under --quiet since it is the
        // command's scripted output.
        return EXIT_OK;
    }
    // Shared run-tail order: failures → publish_failed → mismatches →
    // auth (run::emit_run_failures; per-command events and exit-code
    // arithmetic stay here).
    emit_run_failures(&mut reporter, &tally, &args.model_id, &publish_failures);
    if tally.auth_required {
        return EXIT_AUTH;
    }
    EXIT_FAILURE
}

/// Acquire the per-repo sync lock (plans/hf-cache-sync.md §5.2 step 6), emitting the error event
/// and exit code on failure.
fn acquire_sync_lock_or_fail(
    staging: &Path,
    reporter: &mut Reporter,
) -> Result<crate::cache_layout::SyncLockGuard, i32> {
    crate::cache_layout::acquire_sync_lock(staging).map_err(|e| {
        reporter.emit(&Event::error(
            ErrorCode::SyncLock,
            format!("cannot acquire sync lock: {e}"),
        ));
        EXIT_FAILURE
    })
}

/// Registry hygiene (plans/hf-cache-sync.md §4.6): drop every on-disk registry entry whose
/// `local_path` lives in a `.rhd-staging` directory. The engine records
/// hf-cache fetches there (it cannot tell cache syncs from flat
/// downloads); those paths are renamed away or cleaned at publish, so the
/// entries would dangle forever in the TUI's resume/complete views.
/// Best-effort: registry errors are ignored (the sync itself must not
/// fail because housekeeping did).
fn purge_staging_registry_entries() {
    let mut registry = crate::registry::load_registry();
    let before = registry.downloads.len();
    registry
        .downloads
        .retain(|d| !d.local_path.contains(".rhd-staging"));
    if registry.downloads.len() != before {
        crate::registry::save_registry(&registry);
    }
}
