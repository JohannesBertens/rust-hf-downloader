//! Download orchestration: config → resolve → engine bootstrap → event
//! drain. `engine::bootstrap` owns the startup sequence; the shared
//! config fold, queue handoff, drain loop, and run-tail emissions live in
//! [`super::run`] (Runner).

use std::path::PathBuf;

use super::args::DownloadArgs;
use super::events::{ErrorCode, Event, FileDto, Summary};
use super::report::Reporter;
use super::resolve::{
    flare_package_set, flare_preflight, is_commit_sha, parse_selector, payload_revision_names,
    preferred_revision_name, resolve_files, select_flare_files, selection_error_event,
    FlarePreflightFile, Selector,
};
use super::run::{
    effective_revision, emit_client_error, emit_metadata_error, emit_run_failures,
    emit_token_warning, invalid_model_id_message, load_run_config, monitor, queue_run,
    require_valid_model_id, RunTally,
};
use super::{EXIT_FAILURE, EXIT_INTERRUPTED, EXIT_USAGE};
use crate::engine::{EnqueuePolicy, QueuedDownload};
use crate::models::AppOptions;

pub(super) async fn run_download(args: DownloadArgs) -> i32 {
    let mut reporter = Reporter::new(
        args.run_output.json,
        args.run_output.quiet,
        args.run_output.progress,
    );

    // --- 1. Configuration (Runner fold: run::load_run_config) ------------
    let (options, token, api_client, token_warning) = match load_run_config(
        args.run_output.token.clone(),
        args.output.as_deref(),
        args.rate_limits.rate_limit,
        args.rate_limits.no_rate_limit,
        args.rate_limits.rate_limit_mbps,
        args.run_output.no_verify,
    ) {
        Ok(bootstrap) => bootstrap,
        Err(e) => return emit_client_error(&mut reporter, &e),
    };
    if let Some(warning) = &token_warning {
        emit_token_warning(&mut reporter, warning);
    }

    // The FLARE preset pipeline (plans/flare-models.md §3.3) replaces the
    // ordinary resolve → enqueue flow wholesale: revision ladder, semantic
    // package selection, commit-pinned URLs, revision-scoped destinations.
    // Model-ID validation comes FIRST (impl-gate P2): the flare path builds
    // author/model path components, so a malformed id must get the standard
    // usage error, never a network call or a parts[1] panic.
    if let Err(code) = require_valid_model_id(&args.model_id) {
        reporter.emit(&Event::error(
            ErrorCode::Usage,
            invalid_model_id_message(&args.model_id),
        ));
        return code;
    }
    if args.for_preset.is_some() {
        return run_download_flare(&args, &mut reporter, &options, &token, &api_client).await;
    }

    // --- 2. Validate usage ------------------------------------------------
    let revision = effective_revision(&args.revision);
    let selector = match parse_selector(&args) {
        Ok(selector) => selector,
        Err(message) => {
            reporter.emit(&Event::error(ErrorCode::Usage, message));
            return EXIT_USAGE;
        }
    };

    // --- 3. Resolve files ---------------------------------------------------
    let metadata =
        match crate::api::fetch_model_metadata(&api_client, &args.model_id, &revision).await {
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
        // Absent for the ordinary command — the optional fields are the
        // FLARE preset's (plans/flare-models.md §3.3f).
        revision: None,
        commit: None,
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
            remote_path: file.remote_path.clone(),
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
            "interrupted by SIGINT; unfinished files stay registered as incomplete and RESUME from their partial download on the next run".to_string(),
        ));
        return EXIT_INTERRUPTED;
    }
    emit_run_failures(&mut reporter, &tally, &args.model_id, &[]);

    tally.exit_code()
}

// ---------------------------------------------------------------------------
// FLARE preset pipeline (plans/flare-models.md §3.3b/§3.3c/§3.3e)
// ---------------------------------------------------------------------------

/// The resolved FLARE revision pin (plans/flare-models.md §3.3b): the 40-hex commit everything
/// keys on (trees, URLs, the registry), plus the local directory segment.
struct FlarePin {
    commit: String,
    /// Preferred revision name, or the 40-hex sha for an explicitly
    /// pinned run (plans/flare-models.md §3.3b i) — the LOCAL destination prefix (plans/flare-models.md §3.3d) and
    /// the `Event::Resolved` display name.
    revision_dir: String,
}

/// plans/flare-models.md §3.3b ladder for an explicit `--revision` value: (i) a 40-hex value is
/// used DIRECTLY as the pin (no refs validation; tree fetched at the sha;
/// directory segment = the sha); (ii) a branch/tag name present in
/// `/refs` pins to its `targetCommit` (an entry WITHOUT a usable commit
/// falls through — "lacking a commit"); (iii) anything else falls back to
/// `resolve_revision_sha`. Failure is a hard error BEFORE any payload
/// fetch. Emits its own events; `Err` is the exit code.
async fn flare_pin_for_revision(
    reporter: &mut Reporter,
    client: &reqwest::Client,
    model_id: &str,
    revision: &str,
) -> Result<FlarePin, i32> {
    if is_commit_sha(revision) {
        return Ok(FlarePin {
            commit: revision.to_string(),
            revision_dir: revision.to_string(),
        });
    }
    let refs = match crate::api::fetch_repo_refs(client, model_id).await {
        Ok(refs) => refs,
        Err(e) => return Err(emit_metadata_error(reporter, model_id, &e)),
    };
    let named = refs
        .branches
        .iter()
        .chain(refs.tags.iter())
        .find(|entry| entry.name == revision && is_commit_sha(&entry.target_commit));
    if let Some(entry) = named {
        // The preferred name for this commit (branch > tag); the user's
        // name is an alias of the same pin.
        let revision_dir = preferred_revision_name(&refs, &entry.target_commit)
            .unwrap_or_else(|| revision.to_string());
        return Ok(FlarePin {
            commit: entry.target_commit.clone(),
            revision_dir,
        });
    }
    match crate::api::resolve_revision_sha(client, model_id, revision).await {
        Ok(sha) => {
            let revision_dir =
                preferred_revision_name(&refs, &sha).unwrap_or_else(|| revision.to_string());
            Ok(FlarePin {
                commit: sha,
                revision_dir,
            })
        }
        Err(e) => {
            let not_found = e.status() == Some(reqwest::StatusCode::NOT_FOUND);
            reporter.emit(&Event::error(
                ErrorCode::Usage,
                format!("failed to resolve revision {revision:?} for {model_id}: {e}"),
            ));
            Err(if not_found { EXIT_USAGE } else { EXIT_FAILURE })
        }
    }
}

/// plans/flare-models.md §3.3b `--gpus` path: detection (plans/flare-models.md §3.2) resolves through the same ladder —
/// the detected platform selects the payload revision whose package arch
/// token matches, and its `targetCommit` is the pin. GPU errors are
/// repo-agnostic (the gpu module's contract); THIS layer appends the
/// repo's payload revisions from `/refs`.
async fn flare_pin_for_gpus(
    reporter: &mut Reporter,
    client: &reqwest::Client,
    model_id: &str,
    gpus_spec: &str,
) -> Result<FlarePin, i32> {
    let selected = match crate::gpu::parse_gpus_spec(gpus_spec) {
        Ok(selected) => selected,
        Err(message) => {
            reporter.emit(&Event::error(ErrorCode::Usage, message));
            return Err(EXIT_USAGE);
        }
    };
    let refs = match crate::api::fetch_repo_refs(client, model_id).await {
        Ok(refs) => refs,
        Err(e) => return Err(emit_metadata_error(reporter, model_id, &e)),
    };
    let revisions = payload_revision_names(&refs);
    let revision_hint = if revisions.is_empty() {
        "this repository has no payload revisions".to_string()
    } else {
        format!(
            "available payload revisions: {} — pass --revision <name>",
            revisions.join(", ")
        )
    };
    // `detect_platform` shells out to nvidia-smi (sync, 5 s budget) —
    // spawn_blocking per the plans/flare-models.md §3.2 contract.
    let detected =
        match tokio::task::spawn_blocking(move || crate::gpu::detect_platform(&selected)).await {
            Ok(result) => result,
            Err(join_err) => Err(crate::gpu::GpuDetectError::Unparseable(format!(
                "detection task failed: {join_err}"
            ))),
        };
    let platform = match detected {
        Ok(platform) => platform,
        Err(error) => {
            reporter.emit(&Event::error(
                ErrorCode::Usage,
                format!("GPU detection failed: {error}\n{revision_hint}"),
            ));
            return Err(EXIT_USAGE);
        }
    };
    let info = match crate::api::fetch_flare_info(client, model_id).await {
        Ok(info) => info,
        Err(crate::api::FlareScanError::Http(e)) => {
            return Err(emit_metadata_error(reporter, model_id, &e));
        }
        Err(crate::api::FlareScanError::NoPackage) => {
            reporter.emit(&Event::error(
                ErrorCode::Usage,
                format!("no FLARE payload revision yielded a package for {model_id}"),
            ));
            return Err(EXIT_USAGE);
        }
    };
    let matching = crate::api::select_platform_revisions(&info.revisions, &platform);
    match matching.as_slice() {
        [one] => Ok(FlarePin {
            commit: one.target_commit.clone(),
            revision_dir: one.revision.clone(),
        }),
        [] => {
            reporter.emit(&Event::error(
                ErrorCode::Usage,
                format!(
                    "no payload revision of {model_id} targets platform {} — {revision_hint}",
                    platform.as_str()
                ),
            ));
            Err(EXIT_USAGE)
        }
        many => {
            let names: Vec<String> = many.iter().map(|r| r.revision.clone()).collect();
            reporter.emit(&Event::error(
                ErrorCode::Usage,
                format!(
                    "multiple payload revisions of {model_id} target platform {}: {} — pass --revision <name> to disambiguate",
                    platform.as_str(),
                    names.join(", ")
                ),
            ));
            Err(EXIT_USAGE)
        }
    }
}

/// Neither `--revision` nor `--gpus` (plans/flare-models.md §3.3b): a hard error listing the
/// repo's payload revisions from `/refs`.
async fn flare_pin_neither(
    reporter: &mut Reporter,
    client: &reqwest::Client,
    model_id: &str,
) -> Result<FlarePin, i32> {
    let refs = match crate::api::fetch_repo_refs(client, model_id).await {
        Ok(refs) => refs,
        Err(e) => return Err(emit_metadata_error(reporter, model_id, &e)),
    };
    let revisions = payload_revision_names(&refs);
    let listing = if revisions.is_empty() {
        "this repository has no payload revisions".to_string()
    } else {
        format!("available payload revisions: {}", revisions.join(", "))
    };
    reporter.emit(&Event::error(
        ErrorCode::Usage,
        format!("download --for flare needs --revision <name> or --gpus <SPEC>; {listing}"),
    ));
    Err(EXIT_USAGE)
}

/// The `download --for flare` pipeline (plans/flare-models.md §3.3): revision ladder → tree at
/// the commit pin → wave-1 `choose_file` package selection → plans/flare-models.md §3.3c file
/// set (+ include/exclude within it) → commit-pinned enqueue with
/// revision-scoped destinations (plans/flare-models.md §3.3d) → the shared monitor/summary
/// tail. LFS oids forward into `expected_sha256` (plans/flare-models.md §3.3e) so payloads are
/// digest-verified by default.
pub(super) async fn run_download_flare(
    args: &DownloadArgs,
    reporter: &mut Reporter,
    options: &AppOptions,
    token: &Option<String>,
    api_client: &reqwest::Client,
) -> i32 {
    // --- 1. Revision ladder (plans/flare-models.md §3.3b) --------------------------------------
    let pin = if let Some(revision) = args.revision.as_deref() {
        flare_pin_for_revision(reporter, api_client, &args.model_id, revision).await
    } else if let Some(spec) = args.gpus.as_deref() {
        flare_pin_for_gpus(reporter, api_client, &args.model_id, spec).await
    } else {
        flare_pin_neither(reporter, api_client, &args.model_id).await
    };
    let pin = match pin {
        Ok(pin) => pin,
        // Ladder errors (and their exit codes) were emitted above.
        Err(code) => {
            reporter.finish();
            return code;
        }
    };

    // --- 2. Tree + package at the pin ------------------------------------
    let tree = match crate::api::fetch_revision_tree(api_client, &args.model_id, &pin.commit).await
    {
        Ok(tree) => tree,
        Err(e) => {
            reporter.finish();
            return emit_metadata_error(reporter, &args.model_id, &e);
        }
    };
    let package = match crate::api::select_package(&tree) {
        Some(package) => package,
        None => {
            reporter.emit(&Event::error(
                ErrorCode::Usage,
                format!(
                    "no FLARE package at revision {} ({}) — flare's choose-file rule (one top-level packed .safetensors, or a directory package with model.safetensors) matched nothing",
                    pin.revision_dir, pin.commit
                ),
            ));
            reporter.finish();
            return EXIT_USAGE;
        }
    };

    // --- 3. plans/flare-models.md §3.3c file set + include/exclude WITHIN it -------------------
    let package_set = flare_package_set(&package);
    let files = match select_flare_files(
        &package_set,
        &pin.revision_dir,
        &args.include,
        &args.exclude,
    ) {
        Ok(files) => files,
        Err(err) => {
            reporter.emit(&selection_error_event(
                &err,
                err.available().iter().map(FileDto::from).collect(),
            ));
            reporter.finish();
            return EXIT_USAGE;
        }
    };

    let total_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
    reporter.emit(&Event::Resolved {
        model: args.model_id.clone(),
        files: files.iter().map(FileDto::from).collect(),
        total_bytes,
        revision: Some(pin.revision_dir.clone()),
        commit: Some(pin.commit.clone()),
    });

    // --- 4. Destinations + plans/flare-models.md §3.3d pre-flight + the shared engine path -----
    let base = options.default_directory.clone();
    let parts: Vec<&str> = args.model_id.split('/').collect();
    let model_path = PathBuf::from(&base).join(parts[0]).join(parts[1]);

    // plans/flare-models.md §3.3d pre-flight guards, PURE over the on-disk registry (reads take
    // no lock) + on-disk facts, BEFORE enqueue: size parity, moved-branch
    // (scoped to this run's revision dir), single-destination-per-package.
    let preflight_files: Vec<FlarePreflightFile> = files
        .iter()
        .map(|file| FlarePreflightFile {
            filename: file.filename.clone(),
            remote_path: file.remote_path.clone(),
            expected_size: file.size_bytes,
        })
        .collect();
    if let Err(conflict) = flare_preflight(
        &crate::registry::read_registry(),
        &base,
        &args.model_id,
        &pin.revision_dir,
        &pin.commit,
        &preflight_files,
    ) {
        reporter.emit(&Event::error(ErrorCode::Usage, conflict.to_string()));
        reporter.finish();
        return EXIT_USAGE;
    }

    let queued: Vec<QueuedDownload> = files
        .iter()
        .map(|file| QueuedDownload {
            model_id: args.model_id.clone(),
            // THE COMMIT is the pin everywhere (plans/flare-models.md §3.3b): download URLs are
            // resolve_url(model, remote_path, target_commit) and the
            // registry stores the sha.
            revision: pin.commit.clone(),
            filename: file.filename.clone(),
            base_path: model_path.clone(),
            expected_sha256: file.sha256.clone(),
            hf_token: token.clone(),
            total_size: file.size_bytes,
            remote_path: file.remote_path.clone(),
        })
        .collect();
    let (state, manager, outcome) = queue_run(&queued, &EnqueuePolicy::cli_download(&base)).await;
    if let Some(err) = outcome.aborted {
        reporter.emit(&Event::error(ErrorCode::InvalidPath, err.to_string()));
        reporter.finish();
        return EXIT_FAILURE;
    }

    // --- 5. Monitor + summary (the shared tail) ---------------------------
    let mut tally = RunTally {
        files: files.len(),
        total_bytes,
        ..RunTally::default()
    };
    let interrupted = monitor(&state, manager, &files, &mut tally, reporter).await;

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

    if interrupted {
        // plans/flare-models.md §3.3f: the handoff is suppressed on interruption.
        reporter.finish();
        reporter.emit(&Event::error(
            ErrorCode::Interrupted,
            "interrupted by SIGINT; unfinished files stay registered as incomplete and RESUME from their partial download on the next run".to_string(),
        ));
        return EXIT_INTERRUPTED;
    }
    emit_run_failures(reporter, &tally, &args.model_id, &[]);

    // --- 6. plans/flare-models.md §3.3f gated handoff (human reporter only) ---------------------
    // Gate: run success AND every package file Downloaded/AlreadyExists
    // AND the FULL package set (no --include/--exclude filtering).
    let every_file_done = tally.outcomes.len() == files.len()
        && tally.outcomes.iter().all(|outcome| {
            matches!(
                outcome,
                crate::models::FileOutcome::Complete { .. }
                    | crate::models::FileOutcome::AlreadyExists { .. }
            )
        });
    let run_ok = tally.exit_code() == super::EXIT_OK && every_file_done;
    let filtered = !args.include.is_empty() || !args.exclude.is_empty();
    if let Some(line) = flare_handoff_line(
        &package,
        filtered,
        run_ok,
        &model_path,
        &pin.revision_dir,
        &package_set[0].relpath,
        args.gpus.as_deref(),
    ) {
        reporter.handoff_line(&line);
    }
    reporter.finish();
    tally.exit_code()
}

// ---------------------------------------------------------------------------
// plans/flare-models.md §3.3f serve-path handoff (D1)
// ---------------------------------------------------------------------------

/// POSIX single-quote escaping for the handoff path; a no-op on Windows
/// (plans/flare-models.md §3.3f: "paths single-quote-escaped (POSIX) with a Windows no-op").
fn quote_posix_path(path: &std::path::Path) -> String {
    if cfg!(windows) {
        path.display().to_string()
    } else {
        format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
    }
}

/// POSIX single-quote escaping for the `--gpus` spec echo (impl-gate fix:
/// a spec like `0, 1` contains shell-breaking whitespace and would split
/// into two arguments if echoed raw).
fn quote_posix_spec(spec: &str) -> String {
    if cfg!(windows) {
        spec.to_string()
    } else {
        format!("'{}'", spec.replace('\'', r"'\''"))
    }
}

/// The plans/flare-models.md §3.3f gated handoff line — PURE (unit-tested below). `None` =
/// suppressed (interruption or any failure). The partial-selection note
/// replaces the command when `--include`/`--exclude` filtered the package
/// set. Packed form: `flare run <quoted final path of the primary> [--gpus
/// <spec verbatim>]` — the path from the pure `download::final_path_for`
/// (parity-pinned with the engine). Directory form: explicit-path
/// acceptance of a directory package is VERIFIED against flare's source
/// (`PackageClaim::read` / `Metadata::open` join `model.safetensors` for
/// `is_dir` paths — see the plans/flare-models.md §3.3f verification line in
/// plans/flare-models.md), so the directory itself is handed off.
fn flare_handoff_line(
    package: &crate::models::FlarePackage,
    filtered: bool,
    run_ok: bool,
    model_root: &std::path::Path,
    revision_dir: &str,
    primary_relpath: &str,
    gpus_spec: Option<&str>,
) -> Option<String> {
    if !run_ok {
        return None; // suppressed on interruption or any failure
    }
    if filtered {
        return Some("partial package selection — no serve command".to_string());
    }
    // Omitted when the user passed none (flare defaults to the first GPU).
    let gpus = gpus_spec
        .map(|spec| format!(" --gpus {}", quote_posix_spec(spec)))
        .unwrap_or_default();
    match package {
        crate::models::FlarePackage::Packed { .. } => {
            let path = crate::download::final_path_for(
                model_root,
                &format!("{revision_dir}/{primary_relpath}"),
            )?;
            Some(format!("flare run {}{}", quote_posix_path(&path), gpus))
        }
        crate::models::FlarePackage::Directory { .. } => {
            let dir = model_root.join(revision_dir);
            Some(format!("flare run {}{}", quote_posix_path(&dir), gpus))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FlarePackage, RepoFile};

    fn packed() -> FlarePackage {
        FlarePackage::Packed {
            file: RepoFile {
                rfilename: "qwen3.8-flash-next-gb10-tp1.safetensors".to_string(),
                size: Some(1),
                oid: None,
                lfs: None,
            },
        }
    }

    fn directory() -> FlarePackage {
        FlarePackage::Directory {
            primary: RepoFile {
                rfilename: "model.safetensors".to_string(),
                size: Some(1),
                oid: None,
                lfs: None,
            },
            companions: Vec::new(),
        }
    }

    /// Packed: quoted final path of the primary (via the pure
    /// `final_path_for`), `--gpus` echoed verbatim when present, omitted
    /// when absent.
    #[test]
    fn handoff_packed_quotes_path_and_echoes_gpus_verbatim() {
        let root = std::path::Path::new("/data/models/org/model");
        let line = flare_handoff_line(
            &packed(),
            false,
            true,
            root,
            "gb10",
            "qwen3.8-flash-next-gb10-tp1.safetensors",
            Some("0,1-2"),
        )
        .unwrap();
        assert_eq!(
            line,
            "flare run '/data/models/org/model/gb10/qwen3.8-flash-next-gb10-tp1.safetensors' --gpus '0,1-2'"
        );

        let no_gpus = flare_handoff_line(
            &packed(),
            false,
            true,
            root,
            "gb10",
            "qwen3.8-flash-next-gb10-tp1.safetensors",
            None,
        )
        .unwrap();
        assert_eq!(
            no_gpus,
            "flare run '/data/models/org/model/gb10/qwen3.8-flash-next-gb10-tp1.safetensors'"
        );
    }

    /// A `--gpus` spec containing whitespace (`0, 1`) is single-quoted in
    /// the echo (impl-gate quoting fix — mirrors the two expectations
    /// above): echoed raw it would split into two shell arguments.
    #[test]
    fn handoff_gpus_spec_with_whitespace_is_single_quoted() {
        let line = flare_handoff_line(
            &packed(),
            false,
            true,
            std::path::Path::new("/data/models/org/model"),
            "gb10",
            "qwen3.8-flash-next-gb10-tp1.safetensors",
            Some("0, 1"),
        )
        .unwrap();
        assert_eq!(
            line,
            "flare run '/data/models/org/model/gb10/qwen3.8-flash-next-gb10-tp1.safetensors' --gpus '0, 1'"
        );
    }

    /// Directory: the package DIRECTORY is handed off (verified against
    /// flare's explicit-path source — see the plan's plans/flare-models.md §3.3f line).
    #[test]
    fn handoff_directory_serves_the_package_dir() {
        let line = flare_handoff_line(
            &directory(),
            false,
            true,
            std::path::Path::new("/data/models/org/model"),
            "gb202",
            "model.safetensors",
            Some("0"),
        )
        .unwrap();
        assert_eq!(line, "flare run '/data/models/org/model/gb202' --gpus '0'");
    }

    /// Partial selection (--include/--exclude present) replaces the command
    /// with the plans/flare-models.md §3.3f note — even on a fully successful run.
    #[test]
    fn handoff_partial_selection_prints_the_note() {
        let line = flare_handoff_line(
            &packed(),
            true,
            true,
            std::path::Path::new("/data"),
            "gb10",
            "f.safetensors",
            Some("0"),
        )
        .unwrap();
        assert_eq!(line, "partial package selection — no serve command");
    }

    /// Failure/interruption suppresses the handoff entirely (no note).
    #[test]
    fn handoff_suppressed_on_failure() {
        assert!(flare_handoff_line(
            &packed(),
            false,
            false,
            std::path::Path::new("/data"),
            "gb10",
            "f.safetensors",
            Some("0"),
        )
        .is_none());
        // The conflict-error case takes the same shape at the pipeline
        // level: a plans/flare-models.md §3.3d pre-flight conflict returns before enqueue, so no
        // run ever reaches the handoff with `run_ok` semantics — pinned by
        // the guard fixtures in resolve_tests.
    }

    /// POSIX single-quote escaping round-trips an embedded quote; the
    /// Windows branch is a no-op (cfg-gated, pinned by the cfg! branch).
    #[test]
    fn handoff_path_quoting_escapes_embedded_single_quotes() {
        assert_eq!(
            quote_posix_path(std::path::Path::new("/it's/a path")),
            if cfg!(windows) {
                "/it's/a path".to_string()
            } else {
                "'/it'\\''s/a path'".to_string()
            }
        );
    }
}
