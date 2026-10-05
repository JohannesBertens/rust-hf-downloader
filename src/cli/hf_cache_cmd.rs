//! `hf-cache` subcommand group (plans/hf-cache-sync.md §2, §5.2): the
//! hub-cache sync pipeline and pure path helper.

use super::args::{
    apply_rate_limit_overrides, merge_token, valid_model_id, HfCacheArgs, HfCacheCommand,
    HfCachePathArgs, HfCacheSyncArgs,
};
use super::download_cmd::{monitor, RunTally};
use super::events::{Event, FileDto, Summary};
use super::report::{truncate_path, Reporter};
use super::resolve::FileSpec;
use super::{EXIT_AUTH, EXIT_FAILURE, EXIT_INTERRUPTED, EXIT_OK, EXIT_USAGE};
use crate::engine::EngineState;
use crate::models::{FileOutcome, ModelMetadata, VerifyOutcome};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

/// Whole-repo sync hint (§2.2 precedence step 4).
const TIP_USE_FOR_VLLM: &str = "tip: use --for vllm to fetch only what vLLM reads";

/// Dispatch the `hf-cache` subcommand group.
pub(super) async fn run_hf_cache(args: HfCacheArgs) -> i32 {
    match args.command {
        HfCacheCommand::Sync(args) => run_hf_cache_sync(args).await,
        HfCacheCommand::Path(args) => run_hf_cache_path(args).await,
    }
}

// --- selection (§2.2 precedence; pure, unit-testable) ---------------------

/// How a sync's file selection was derived (§2.2) — `WholeRepo` triggers
/// the `--for vllm` tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// Explicit positional `FILE…` — exactly those files.
    Files,
    /// `--include`/`--exclude` globs over the full tree.
    Patterns,
    /// A `--for <PRESET>` allow/ignore table (§2.3).
    Preset,
    /// No selector: the whole repository (hf `download` parity).
    WholeRepo,
}

/// Selection failures (§2.2): all map to [`EXIT_USAGE`] with the full
/// structured file list attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncSelectionError {
    /// A positional `FILE` is not present in the repository tree.
    MissingPositional {
        path: String,
        available: Vec<String>,
    },
    /// `--for` named a preset this binary does not know (unreachable via
    /// clap's `parse_preset`; kept for direct callers).
    UnknownPreset { name: String },
    /// Every mode plus `--exclude` filtering left nothing to sync.
    EmptySelection { available: Vec<String> },
}

impl SyncSelectionError {
    pub(super) fn code(&self) -> &'static str {
        match self {
            SyncSelectionError::MissingPositional { .. } => "no_files_match",
            SyncSelectionError::UnknownPreset { .. } => "unknown_preset",
            SyncSelectionError::EmptySelection { .. } => "empty_selection",
        }
    }

    pub(super) fn message(&self) -> String {
        match self {
            SyncSelectionError::MissingPositional { path, .. } => {
                format!("file not present in repository: {path}")
            }
            SyncSelectionError::UnknownPreset { name } => {
                format!("unknown preset {name:?} — available presets: vllm")
            }
            SyncSelectionError::EmptySelection { .. } => {
                "selection matched no files in the repository".to_string()
            }
        }
    }
}

/// Resolve the files a sync targets, per the §2.2 precedence:
///
/// 1. positional `FILE…` → exactly those files (each must exist in the
///    tree; duplicates collapse, order preserved),
/// 2. else `--include`/`--exclude` → Python-fnmatch globs over the full
///    tree (`*` crosses `/`, §7),
/// 3. else `--for vllm` → the preset allow/ignore table from §2.3
///    ([`crate::patterns::VLLM_ALLOW`]/[`crate::patterns::VLLM_IGNORE`]),
/// 4. else the whole repository ([`SelectionMode::WholeRepo`] — the
///    caller prints the `--for vllm` tip).
///
/// `--exclude` applies on top of every mode (§2.2). An empty selection
/// after all filtering is an error carrying the available file list.
pub fn select_sync_files(
    tree: &[&str],
    files: &[String],
    include: &[String],
    exclude: &[String],
    preset: Option<&str>,
) -> Result<(Vec<String>, SelectionMode), SyncSelectionError> {
    let available = || {
        tree.iter()
            .map(|path| (*path).to_string())
            .collect::<Vec<_>>()
    };
    let (mut selected, mode) = if !files.is_empty() {
        let mut picked: Vec<String> = Vec::with_capacity(files.len());
        for file in files {
            if !tree.contains(&file.as_str()) {
                return Err(SyncSelectionError::MissingPositional {
                    path: file.clone(),
                    available: available(),
                });
            }
            if !picked.contains(file) {
                picked.push(file.clone());
            }
        }
        (picked, SelectionMode::Files)
    } else if !include.is_empty() {
        let picked = crate::patterns::filter_paths(tree, include, exclude)
            .into_iter()
            .map(String::from)
            .collect();
        (picked, SelectionMode::Patterns)
    } else if let Some(preset) = preset {
        match preset {
            "vllm" => {
                let allow: Vec<String> = crate::patterns::VLLM_ALLOW
                    .iter()
                    .map(|pattern| pattern.to_string())
                    .collect();
                // The preset's own ignore table filters here; a user
                // --exclude is additionally applied by the universal
                // post-filter below (§2.2: exclude applies on top of every
                // mode).
                let ignore: Vec<String> = crate::patterns::VLLM_IGNORE
                    .iter()
                    .map(|pattern| pattern.to_string())
                    .collect();
                let picked = crate::patterns::filter_paths(tree, &allow, &ignore)
                    .into_iter()
                    .map(String::from)
                    .collect();
                (picked, SelectionMode::Preset)
            }
            other => {
                return Err(SyncSelectionError::UnknownPreset {
                    name: other.to_string(),
                })
            }
        }
    } else {
        (available(), SelectionMode::WholeRepo)
    };

    // §2.2: --exclude applies on top of every mode (filter_paths already
    // applied it for the Patterns/Preset paths; re-applying is idempotent).
    if !exclude.is_empty() {
        selected.retain(|path| {
            !exclude
                .iter()
                .any(|pattern| crate::patterns::fnmatch(pattern, path))
        });
    }

    if selected.is_empty() {
        return Err(SyncSelectionError::EmptySelection {
            available: available(),
        });
    }
    Ok((selected, mode))
}

// --- small pure helpers ----------------------------------------------------

/// The `refs/` name for a revision (R2): branch/tag revisions are written
/// to `refs/<name>`; a raw 40-hex commit-SHA revision gets **no** ref (hub
/// behavior — the snapshot is addressed by SHA alone).
pub(super) fn ref_name_for_revision(revision: &str) -> Option<&str> {
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
pub(super) fn symlinks_enabled(no_symlinks_flag: bool, env_value: Option<&str>) -> bool {
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

/// Absolute form of `path` without canonicalization's symlink resolution:
/// already-absolute paths pass through verbatim, relative paths anchor at
/// the current directory. §2.4's "last line: the snapshot path" wants a
/// stable, predictable absolute path (containers mount caches elsewhere).
pub(super) fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
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

/// FileDto listing of a repo tree (selection-error `available` payloads,
/// mirroring `resolve_files`' ambiguity lists).
fn tree_file_dtos(metadata: &ModelMetadata) -> Vec<FileDto> {
    metadata
        .siblings
        .iter()
        .filter(|f| !f.rfilename.ends_with('/'))
        .map(|f| FileDto {
            filename: f.rfilename.clone(),
            size_bytes: f.size.unwrap_or(0),
            sha256: f.lfs.as_ref().map(|lfs| lfs.oid.clone()),
        })
        .collect()
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
        if let Err(e) = crate::hf_cache::ensure_snapshot_entry(repo_dir, sha, file, use_symlinks) {
            return Err(format!("cannot link snapshot entry for {path}: {e}"));
        }
    }
    Ok(())
}

/// Human dry-run table (§5.2 step 4): per-file fetch/cached rows with
/// sizes (hf CLI parity). `--json` mode prints only the `SyncPlanned`
/// event, so this is human-only output.
fn print_sync_dry_run(
    model: &str,
    revision: &str,
    sha: &str,
    plan: &crate::hf_cache::SyncPlan,
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
            truncate_path(&item.repo_path, 58),
            crate::utils::format_size(item.size)
        );
    }
    for path in &plan.up_to_date {
        let _ = writeln!(
            out,
            " {:<6} {:<58} {:>9}",
            "cached",
            truncate_path(path, 58),
            crate::utils::format_size(size_of(path))
        );
    }
    let _ = writeln!(
        out,
        "{} file(s) to fetch ({}), {} already cached — dry run, nothing written",
        plan.fetch.len(),
        crate::utils::format_size(fetch_bytes),
        plan.up_to_date.len()
    );
    let _ = out.flush();
}

// --- `hf-cache sync` (§5.2 pipeline, followed exactly) ---------------------

async fn run_hf_cache_sync(args: HfCacheSyncArgs) -> i32 {
    let mut reporter = Reporter::new(args.json, args.quiet, args.progress);

    // --- 1. Configuration (mirrors run_download, minus the output-dir
    //        override: the destination is the hub cache, §4.1) -------------
    let mut options = crate::config::load_config();
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

    // --- 2. Validate usage (§5.2 step 1: revision already parsed by
    //        clap's parse_revision) ----------------------------------------
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
    let revision = args
        .revision
        .clone()
        .unwrap_or_else(|| crate::api::DEFAULT_REVISION.to_string());

    // --- 3. Tree + commit SHA in parallel (§5.2 step 2) --------------------
    let (metadata_res, sha_res) = tokio::join!(
        crate::api::fetch_model_metadata(&args.model_id, &revision, token.as_ref()),
        crate::api::resolve_revision_sha(&args.model_id, &revision, token.as_ref()),
    );
    let metadata = match metadata_res {
        Ok(metadata) => metadata,
        Err(e) => {
            let not_found = e.status() == Some(reqwest::StatusCode::NOT_FOUND);
            reporter.emit(&Event::Error {
                code: if not_found { "not_found" } else { "network" }.to_string(),
                message: format!("failed to fetch model info for {}: {}", args.model_id, e),
                available: None,
            });
            return if not_found { EXIT_USAGE } else { EXIT_FAILURE };
        }
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
            // Unknown revision (404) is a usage error (§2.4).
            return if not_found { EXIT_USAGE } else { EXIT_FAILURE };
        }
    };

    // --- 4. Selection precedence (§2.2) -------------------------------------
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
            reporter.emit(&Event::Error {
                code: err.code().to_string(),
                message: err.message(),
                available: Some(tree_file_dtos(&metadata)),
            });
            return EXIT_USAGE;
        }
    };
    if mode == SelectionMode::WholeRepo {
        reporter.status_line(TIP_USE_FOR_VLLM);
    }

    // --- 5. Plan against the current cache (R6) + SyncPlanned (§2.4) --------
    let cache_dir = crate::paths::hf_hub_cache(args.cache_dir.as_deref());
    let plan = match crate::hf_cache::plan(
        &cache_dir,
        &args.model_id,
        &metadata.siblings,
        &selected,
        &sha,
        args.force,
    ) {
        Ok(plan) => plan,
        Err(e) => {
            reporter.emit(&Event::Error {
                code: "plan_failed".to_string(),
                message: format!("cannot plan cache sync for {}: {}", args.model_id, e),
                available: None,
            });
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

    // --- 6. Dry run: the plan is the output; no writes (§5.2 step 4) --------
    if args.dry_run {
        if !args.json {
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

    // --- 7. Fully-cached no-op (§5.2 step 3): write refs, relink entries,
    //        print the snapshot path, exit 0.
    let repo_dir = cache_dir.join(crate::hf_cache::repo_dir_name(&args.model_id));
    let staging = crate::hf_cache::staging_dir(&repo_dir);
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
            reporter.emit(&Event::Error {
                code: "io".to_string(),
                message,
                available: None,
            });
            return EXIT_FAILURE;
        }
        if let Err(e) =
            crate::hf_cache::write_refs(&repo_dir, ref_name_for_revision(&revision), &sha)
        {
            reporter.emit(&Event::Error {
                code: "io".to_string(),
                message: format!("cannot write refs: {e}"),
                available: None,
            });
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
        let snapshot_path = absolute_path(&crate::hf_cache::snapshot_dir(
            &cache_dir,
            &args.model_id,
            &sha,
        ));
        // SyncComplete's human rendering is the snapshot path itself —
        // the last line, hf CLI parity (§2.4).
        reporter.emit(&Event::SyncComplete {
            snapshot_path: snapshot_path.display().to_string(),
            revision: revision.clone(),
            sha: sha.clone(),
        });
        reporter.finish();
        return EXIT_OK;
    }

    // --- 8. Layout dirs + CACHEDIR.TAG (§5.2 step 5) -------------------------
    let snapshot_root = crate::hf_cache::snapshot_dir(&cache_dir, &args.model_id, &sha);
    for dir in [repo_dir.join("blobs"), snapshot_root, staging.clone()] {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            reporter.emit(&Event::Error {
                code: "io".to_string(),
                message: format!("cannot create {}: {}", dir.display(), e),
                available: None,
            });
            return EXIT_FAILURE;
        }
    }
    if let Err(e) = crate::paths::write_cachedir_tag(&cache_dir) {
        reporter.emit(&Event::Error {
            code: "io".to_string(),
            message: format!(
                "cannot write CACHEDIR.TAG in {}: {}",
                cache_dir.display(),
                e
            ),
            available: None,
        });
        return EXIT_FAILURE;
    }

    // --- 9. Sync lock (§5.2 step 6) ------------------------------------------
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
        reporter.emit(&Event::Error {
            code: "io".to_string(),
            message,
            available: None,
        });
        return EXIT_FAILURE;
    }

    // --- 10. Engine bootstrap, exactly like run_download (§5.2 step 7) -------
    // Note: engine::register_pending is deliberately NOT called — the
    // flat-download registry is TUI-resume state (§4.6). The engine still
    // writes registry entries for files it fetches (staging paths); those
    // are swept after the run and at the start of the next one so the
    // TUI's resume/complete views stay clean.
    purge_staging_registry_entries();
    let (state, download_tx) = EngineState::new();
    // Load the on-disk registry into the engine mirror (parity with the
    // TUI's startup scan) so verification updates find their entries.
    {
        let mut mirror = state.download_registry.lock().await;
        *mirror = crate::registry::load_registry();
    }
    crate::engine::spawn_verification_worker(state.clone());
    let manager = crate::engine::spawn_manager(state.clone());

    let files: Vec<FileSpec> = plan
        .fetch
        .iter()
        .map(|item| FileSpec {
            filename: item.repo_path.clone(),
            size_bytes: item.size,
            sha256: item.sha256.clone(),
        })
        .collect();
    // Queue accounting + sends (mirrors run_download's confirm_download)
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
    for item in &plan.fetch {
        // base_path = staging dir, filename = repo path (§5.1); the
        // revision is the resolved commit SHA, so a moving branch cannot
        // race the plan. Expected sha = LFS oid; total size from the tree.
        let _ = download_tx.send((
            args.model_id.clone(),
            sha.clone(),
            item.repo_path.clone(),
            staging.clone(),
            item.sha256.clone(),
            token.clone(),
            item.size,
        ));
    }
    // Dropping the sender closes the channel — the manager drains, then
    // its join handle resolves. This is the deterministic completion signal.
    drop(download_tx);

    // --- 11. Monitor until drained (§5.2 step 8, reusing run_download's
    //         monitor/verification-idle machinery) ---------------------------
    let mut tally = RunTally {
        files: selected.len(),
        total_bytes,
        ..RunTally::default()
    };
    let interrupted = monitor(&state, manager, &files, &mut tally, &mut reporter).await;

    // --- 12. Publish gate (§5.2 step 9) --------------------------------------
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
        // Registry hygiene (§4.6): the engine records every fetch in the
        // flat-download registry with staging paths; sweep those entries
        // so the TUI's resume/complete views stay clean. Done here — after
        // the verification drain — because verification updates land in
        // the registry too.
        purge_staging_registry_entries();
        // Verification gate (R5): publish only with no hub digest,
        // verification deliberately skipped, or an explicit Ok.
        match &item.sha256 {
            None => {}
            Some(_) if args.no_verify => {
                reporter.status_line(&format!(
                    "Warning: publishing {} without SHA256 verification (--no-verify)",
                    item.repo_path
                ));
            }
            Some(_) if !verification_active => {} // standing config choice
            Some(_) => match verify_of.get(item.repo_path.as_str()) {
                Some(VerifyOutcome::Ok { .. }) => {}
                Some(VerifyOutcome::Mismatch { .. }) => {
                    // The bad bytes never enter the cache (§2.4).
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
        match crate::hf_cache::publish_one(&repo_dir, &sha, item, &staged, use_symlinks) {
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

    // --- 13. refs + staging cleanup (§5.2 step 10) ----------------------------
    let mut failed = tally.failed > 0
        || tally.hash_mismatch > 0
        || tally.auth_required
        || !publish_failures.is_empty();
    if !interrupted && !failed {
        // R2: refs only for branch/tag revisions, never raw SHAs.
        if let Err(e) =
            crate::hf_cache::write_refs(&repo_dir, ref_name_for_revision(&revision), &sha)
        {
            publish_failures.push(format!("refs/{}: {}", revision, e));
            failed = true;
        } else {
            // Full success: drop staging remnants of published files;
            // .incomplete files of failed runs keep their resume value.
            let _ = crate::hf_cache::cleanup_staging(&repo_dir, &published);
        }
    }

    // --- 14. Summary + snapshot path (§5.2 step 11, §2.4) ---------------------
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

    let snapshot_path = absolute_path(&crate::hf_cache::snapshot_dir(
        &cache_dir,
        &args.model_id,
        &sha,
    ));
    if interrupted {
        reporter.emit(&Event::Error {
            code: "interrupted".to_string(),
            message: "interrupted by SIGINT; staged partial files resume on the next run"
                .to_string(),
            available: None,
        });
        return EXIT_INTERRUPTED;
    }
    if !failed {
        reporter.emit(&Event::SyncComplete {
            snapshot_path: snapshot_path.display().to_string(),
            revision: revision.clone(),
            sha: sha.clone(),
        });
        // SyncComplete's human rendering IS the last line (§2.4) — the
        // snapshot path, printed even under --quiet since it is the
        // command's scripted output.
        return EXIT_OK;
    }
    if !tally.failures.is_empty() {
        reporter.emit(&Event::Error {
            code: "download_failed".to_string(),
            message: tally.failures.join("; "),
            available: None,
        });
    }
    if !publish_failures.is_empty() {
        reporter.emit(&Event::Error {
            code: "publish_failed".to_string(),
            message: publish_failures.join("; "),
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
        return EXIT_AUTH;
    }
    EXIT_FAILURE
}

/// Acquire the per-repo sync lock (§5.2 step 6), emitting the error event
/// and exit code on failure.
fn acquire_sync_lock_or_fail(
    staging: &Path,
    reporter: &mut Reporter,
) -> Result<crate::hf_cache::SyncLockGuard, i32> {
    crate::hf_cache::acquire_sync_lock(staging).map_err(|e| {
        reporter.emit(&Event::Error {
            code: "sync_lock".to_string(),
            message: format!("cannot acquire sync lock: {e}"),
            available: None,
        });
        EXIT_FAILURE
    })
}

/// Registry hygiene (§4.6): drop every on-disk registry entry whose
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

// --- `hf-cache path` (§2.1: pure path math + refs lookup) ------------------

async fn run_hf_cache_path(args: HfCachePathArgs) -> i32 {
    if !valid_model_id(&args.model_id) {
        eprintln!(
            "error [usage]: invalid model ID {:?} — expected \"author/model-name\"",
            args.model_id
        );
        return EXIT_USAGE;
    }
    let revision = args
        .revision
        .clone()
        .unwrap_or_else(|| crate::api::DEFAULT_REVISION.to_string());
    let cache_dir = crate::paths::hf_hub_cache(args.cache_dir.as_deref());
    let repo_dir = cache_dir.join(crate::hf_cache::repo_dir_name(&args.model_id));

    // refs/<rev> lookup: pure path math, no network (§2.1).
    if let Ok(sha) = std::fs::read_to_string(repo_dir.join("refs").join(&revision)) {
        let sha = sha.trim();
        if !sha.is_empty() {
            let snapshot = absolute_path(&crate::hf_cache::snapshot_dir(
                &cache_dir,
                &args.model_id,
                sha,
            ));
            if !snapshot.is_dir() {
                eprintln!(
                    "note: snapshot directory is missing from the cache; \
                     re-run the sync to repair it"
                );
            }
            println!("{}", snapshot.display());
            return EXIT_OK;
        }
    }

    // Online fallback: resolve the revision to a commit SHA.
    let options = crate::config::load_config();
    let token = merge_token(
        args.token.clone(),
        std::env::var("HF_TOKEN").ok(),
        options.hf_token.clone(),
    );
    match crate::api::resolve_revision_sha(&args.model_id, &revision, token.as_ref()).await {
        Ok(sha) => {
            let snapshot = absolute_path(&crate::hf_cache::snapshot_dir(
                &cache_dir,
                &args.model_id,
                &sha,
            ));
            if !snapshot.is_dir() {
                eprintln!(
                    "note: snapshot not present in the cache yet; run \
                     `rust-hf-downloader hf-cache sync {} --revision {}` to populate it",
                    args.model_id, revision
                );
            }
            println!("{}", snapshot.display());
            EXIT_OK
        }
        Err(e) => {
            eprintln!(
                "error [network]: cannot resolve revision {} of {}: {}",
                revision, args.model_id, e
            );
            eprintln!(
                "hint: run `rust-hf-downloader hf-cache sync {}` to populate the cache first",
                args.model_id
            );
            EXIT_FAILURE
        }
    }
}
