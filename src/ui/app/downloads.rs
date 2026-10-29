//! Download initiation: the confirm flows (quant group, repository, tree)
//! and resume — thin gather-ers over `EngineState::enqueue` with per-flow
//! policies (W2.1/W4.4). Observable behavior pinned byte-for-byte by the
//! characterization tests at the bottom of this file.
use super::state::App;
use crate::engine::{EnqueueOutcome, EnqueuePolicy, QueuedDownload};
use crate::models::*;
use crate::paths::sanitize::validate_and_sanitize_path;
use crate::registry;
use crate::ui::tree::count_tree_files;
use std::path::PathBuf;
use tui_input::Input;

impl App {
    /// Shared tail of the three confirm flows: surface every filename the
    /// enqueue transaction rejected (reported and NOT queued), then the
    /// per-flow success status or the per-flow failure error — identical
    /// control flow at every call site, only the strings differ.
    ///
    /// When nothing was sent AND at least one file was rejected, the last
    /// per-file `PathError` stays visible instead of the generic failure:
    /// invalid files are no longer queued (audit fix 1a), so an all-invalid
    /// batch would otherwise lose the reason it downloaded nothing.
    fn finish_enqueue(&mut self, outcome: &EnqueueOutcome, success: String, failure: &str) {
        for (filename, err) in &outcome.invalid {
            *self.error.write() = Some(format!("Invalid filename '{}': {}", filename, err));
        }
        if outcome.sent > 0 {
            *self.status.write() = success;
        } else if outcome.invalid.is_empty() {
            // Accepted edge (impl-gate P2): when `sent == 0`, `invalid` is
            // non-empty, AND valid files existed but failed to send (channel
            // closed), the per-file PathError above stays visible and this
            // generic failure is suppressed on purpose — the invalid-file
            // reason is the actionable message, and a send failure only ever
            // means the download manager is gone. No behavior change.
            *self.error.write() = Some(failure.to_string());
        }
    }

    /// Scan registry for incomplete downloads and show resume popup if found
    pub async fn scan_incomplete_downloads(&mut self) {
        // Seed the engine's registry mirror from disk (the shared startup
        // step the CLI's engine::bootstrap also runs) and reuse the same
        // snapshot for the incomplete/complete views — one disk read.
        let registry = crate::engine::seed_registry_mirror(&self.engine).await;

        // Find incomplete downloads
        self.incomplete_downloads = registry::get_incomplete_downloads(&registry);

        // Load complete downloads into memory
        let complete_map = registry::get_complete_downloads(&registry);

        {
            let mut complete = self.engine.complete_downloads.lock().await;
            *complete = complete_map;
        }

        // Show popup if incomplete downloads found
        if !self.incomplete_downloads.is_empty() {
            self.popup_mode = PopupMode::ResumeDownload;
            *self.status.write() = format!(
                "Found {} incomplete download(s)",
                self.incomplete_downloads.len()
            );
        }
    }

    /// Initiate download flow - show download path popup
    pub fn trigger_download(&mut self) {
        // Check which pane is focused to determine what to download
        match self.focused_pane {
            FocusedPane::Models => {
                // Download entire model repository (non-GGUF models in Standard mode)
                if *self.display_mode.read() == crate::models::ModelDisplayMode::Standard {
                    let metadata = self.model_metadata.read().clone();

                    if let Some(meta) = metadata {
                        let file_count = meta.siblings.len();
                        self.download_path_input =
                            Input::default().with_value(self.options.default_directory.clone());
                        self.popup_mode = PopupMode::DownloadPath;
                        *self.status.write() =
                            format!("Download all {} files from repository", file_count);
                    }
                }
            }
            FocusedPane::FileTree => {
                // Download the selected file (or every file under the selected
                // directory) from the Standard-mode tree — issue #25 P5
                let tree = self.file_tree.read().clone();
                let Some(tree) = tree else {
                    return;
                };
                let Some(selected) = self.file_tree_state.selected() else {
                    return;
                };
                let flat = crate::ui::tree::flatten_tree_for_navigation(&tree);
                let Some(node) = flat.get(selected) else {
                    return;
                };

                self.pending_tree_download = Some((node.path.clone(), node.is_dir));
                self.download_path_input =
                    Input::default().with_value(self.options.default_directory.clone());
                self.popup_mode = PopupMode::DownloadPath;
                if node.is_dir {
                    let count = count_tree_files(node);
                    *self.status.write() =
                        format!("Download all {} files under {}", count, node.path);
                } else {
                    *self.status.write() = format!("Download file {}", node.path);
                }
            }
            FocusedPane::QuantizationGroups => {
                // Download entire quantization group
                let quantizations = self.quantizations.read().clone();

                if let Some(selected) = self.quant_list_state.selected() {
                    if selected < quantizations.len() {
                        // Update download path input with current default directory
                        self.download_path_input =
                            Input::default().with_value(self.options.default_directory.clone());
                        self.popup_mode = PopupMode::DownloadPath;
                        *self.status.write() = format!(
                            "Download all {} files in quantization group",
                            quantizations[selected].files.len()
                        );
                    }
                }
            }
            FocusedPane::QuantizationFiles => {
                // Download specific file only
                if let Some(_group_idx) = self.quant_list_state.selected() {
                    if let Some(_file_idx) = self.quant_file_list_state.selected() {
                        self.download_path_input =
                            Input::default().with_value(self.options.default_directory.clone());
                        self.popup_mode = PopupMode::DownloadPath;
                        *self.status.write() = "Download single selected file".to_string();
                    }
                }
            }
            FocusedPane::FlareRevisions => {
                // plans/flare-models.md §3.5: 'd' in Flare mode opens the
                // revision-picker popup — the snapshot is bound to the
                // currently selected model (the scan's owner by the
                // plans/flare-models.md §3.4
                // ownership rule: every selection change clears
                // `flare_info`, so it can only describe the selection).
                let Some(info) = self.flare_info.read().clone() else {
                    return;
                };
                if info.revisions.is_empty() {
                    return;
                }
                let model_id = {
                    let models = self.models.read();
                    let Some(id) = self
                        .list_state
                        .selected()
                        .and_then(|idx| models.get(idx))
                        .map(|model| model.id.clone())
                    else {
                        return;
                    };
                    id
                };
                let count = info.revisions.len();
                self.popup_mode = PopupMode::FlareRevision {
                    model_id,
                    choices: info
                        .revisions
                        .iter()
                        .map(FlareRevisionChoice::from_revision)
                        .collect(),
                    cursor: 0,
                };
                *self.status.write() =
                    format!("Select a FLARE revision to download ({count} available)");
            }
            _ => {}
        }
    }

    /// Complete download with validation - create metadata and queue download
    pub async fn confirm_download(&mut self) {
        // Tree-pane selection (Standard mode): single file or subtree
        if let Some((path, is_dir)) = self.pending_tree_download.take() {
            self.confirm_tree_download(&path, is_dir).await;
            return;
        }

        // Check if we're downloading a full repository (non-GGUF model)
        if self.focused_pane == FocusedPane::Models
            && *self.display_mode.read() == crate::models::ModelDisplayMode::Standard
        {
            self.confirm_repository_download().await;
            return;
        }

        self.confirm_quant_download().await;
    }

    /// Download selected GGUF quantization (single file or entire group)
    pub async fn confirm_quant_download(&mut self) {
        let models = self.models.read().clone();
        let quant_groups = self.quantizations.read().clone();

        let model_selected = self.list_state.selected();
        let quant_selected = self.quant_list_state.selected();

        if let (Some(model_idx), Some(quant_idx)) = (model_selected, quant_selected) {
            if model_idx < models.len() && quant_idx < quant_groups.len() {
                let model = &models[model_idx];
                let group = &quant_groups[quant_idx];

                // Determine which files to download based on focus
                let files_to_download: Vec<QuantizationInfo> = match self.focused_pane {
                    FocusedPane::QuantizationFiles => {
                        // Download only the selected file
                        if let Some(file_idx) = self.quant_file_list_state.selected() {
                            if file_idx < group.files.len() {
                                vec![group.files[file_idx].clone()]
                            } else {
                                vec![]
                            }
                        } else {
                            vec![]
                        }
                    }
                    _ => {
                        // Download all files in the group (default behavior)
                        group.files.clone()
                    }
                };

                if files_to_download.is_empty() {
                    *self.error.write() = Some("No files selected for download".to_string());
                    return;
                }

                let quant = &files_to_download[0];

                let base_path = self.download_path_input.value().to_string();

                // Validate the path to prevent path traversal
                if let Err(e) = validate_and_sanitize_path(&base_path, &model.id, &quant.filename) {
                    *self.error.write() = Some(format!("Invalid path: {}", e));
                    *self.status.write() = "Download cancelled due to invalid path".to_string();
                    return;
                }

                // Calculate model_path as base/author/model_name (without file's subdirectory)
                // The filename may contain subdirectories (e.g., "UD-Q6_K_XL/model.gguf")
                // which will be appended during download, so we don't include them here
                let model_path = model_root(&base_path, &model.id);

                let num_files = files_to_download.len();

                // Queue payload; the registry entries are derived from
                // these same fields by the enqueue transaction below.
                let queued: Vec<QueuedDownload> = files_to_download
                    .iter()
                    .map(|f| QueuedDownload {
                        model_id: model.id.clone(),
                        revision: crate::api::DEFAULT_REVISION.to_string(),
                        filename: f.filename.clone(),
                        base_path: model_path.clone(),
                        expected_sha256: f.sha256.clone(),
                        hf_token: self.options.hf_token.clone(),
                        total_size: f.size,
                        remote_path: None,
                    })
                    .collect();

                // Shared enqueue transaction, GGUF-quant flavor: registry
                // mirror upsert with zero-size entries, queue accounted
                // before the sends, HUD mirror per successful send,
                // failed-send tail rollback. Files whose path fails
                // validation are skipped from the registry AND never
                // queued (audit fix 1a).
                let outcome = self
                    .engine
                    .enqueue(
                        &self.download_tx,
                        &queued,
                        &EnqueuePolicy::tui_quant(&base_path),
                    )
                    .await;

                // Per-flow strings for the shared tail: a multi-file group
                // counts the requested parts, a single file gets the
                // starting-download line.
                let success = if num_files > 1 {
                    format!(
                        "Queued {} parts of {} to {}",
                        num_files,
                        quant.filename,
                        model_path.display()
                    )
                } else {
                    format!(
                        "Starting download of {} to {}",
                        quant.filename,
                        model_path.display()
                    )
                };
                self.finish_enqueue(&outcome, success, "Failed to start download");
            }
        }
    }

    /// Resume all incomplete downloads from registry
    pub async fn resume_incomplete_downloads(&mut self) {
        let count = self.incomplete_downloads.len();
        let hf_token = self.options.hf_token.clone();
        let default_dir = self.options.default_directory.clone();

        // Base-path derivation per entry. Ordinary entries keep the
        // historical rule: files land under base/author/model (without the
        // filename's own subdirectory, e.g. "Q4_1/model.gguf" —
        // download.rs appends it); a malformed model_id falls back to the
        // recorded local_path's parent directory.
        //
        // FLARE-shaped entries (plans/flare-models.md §3.3d TUI resume
        // rule): the recorded `local_path` is
        // AUTHORITATIVE for the base-path derivation, never the live
        // default dir — an interrupted FLARE download resumed under a
        // different default dir must continue at its RECORDED destination,
        // not relocate bytes while the registry keeps the old one. A
        // missing or mismatched recorded root is a hard error: NOTHING
        // enqueues (the user restores the directory or deletes the entry).
        let mut queued: Vec<QueuedDownload> = Vec::with_capacity(count);
        for metadata in &self.incomplete_downloads {
            let base_path = if is_flare_shaped_entry(metadata) {
                match recorded_flare_root(metadata) {
                    Ok(root) => root,
                    Err(why) => {
                        *self.error.write() =
                            Some(format!("Cannot resume {}: {why}", metadata.filename));
                        *self.status.write() = "Resume aborted — restore the recorded directory or delete the registry entry".to_string();
                        return;
                    }
                }
            } else {
                let fallback = PathBuf::from(&metadata.local_path)
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from(&default_dir));
                model_root_or(&default_dir, &metadata.model_id, fallback)
            };
            queued.push(QueuedDownload {
                model_id: metadata.model_id.clone(),
                revision: metadata
                    .revision
                    .clone()
                    .unwrap_or_else(|| crate::api::DEFAULT_REVISION.to_string()),
                filename: metadata.filename.clone(),
                base_path,
                expected_sha256: metadata.expected_sha256.clone(),
                hf_token: hf_token.clone(),
                total_size: metadata.total_size,
                // Persisted on the entry — a FLARE resume re-enqueues the
                // identical disk path AND the unprefixed request URL
                // (plans/flare-models.md §3.3d interrupt/resume round-trip).
                remote_path: metadata.remote_path.clone(),
            });
        }

        // Shared enqueue transaction, resume flavor: no registry writes
        // (the entries already exist), HUD summary per file regardless of
        // send result, queue accounted once after the sends.
        let _ = self
            .engine
            .enqueue(&self.download_tx, &queued, &EnqueuePolicy::tui_resume())
            .await;

        *self.status.write() = format!("Resuming {} incomplete download(s)", count);
        self.incomplete_downloads.clear();
    }

    /// Delete incomplete files and remove from registry
    pub async fn delete_incomplete_downloads(&mut self) {
        let mut deleted = 0;
        let mut errors = Vec::new();

        // The urls to drop from the registry, collected up front.
        let urls: Vec<String> = self
            .incomplete_downloads
            .iter()
            .map(|metadata| metadata.url.clone())
            .collect();

        for metadata in &self.incomplete_downloads {
            // Remove the `.incomplete` file — and, since the resume fix
            // (issue #50), the journal sidecar next to it (always: an
            // orphan journal must not survive a failed `.incomplete`
            // removal either), so the delete flow leaves no stale state.
            // The path helper is the transport's one exported naming
            // scheme — no magic string here.
            let file_path = PathBuf::from(&metadata.local_path);
            let incomplete_path = PathBuf::from(format!("{}.incomplete", file_path.display()));
            let journal_path = crate::download::journal_sidecar_path(&incomplete_path);
            let _ = tokio::fs::remove_file(&journal_path).await;

            match tokio::fs::remove_file(&incomplete_path).await {
                Ok(_) => deleted += 1,
                Err(e) => {
                    errors.push(format!("{}: {}", metadata.filename, e));
                }
            }
        }

        // Registry write through the typed op (M1 — this used to clone the
        // engine mirror and save it, the R1 bypass site): the op loads the
        // on-DISK registry (never the mirror), drops the selected urls,
        // saves atomically under the registry single writer, and returns
        // the post-write snapshot — the mirror is replaced from it here,
        // after the writer released.
        let snapshot = registry::delete_incomplete_by_urls(&urls);
        *self.engine.download_registry.lock().await = snapshot;

        if errors.is_empty() {
            *self.status.write() = format!("Deleted {} incomplete file(s)", deleted);
        } else {
            *self.status.write() = format!(
                "Deleted {} file(s), {} error(s): {}",
                deleted,
                errors.len(),
                errors.join(", ")
            );
        }
        self.incomplete_downloads.clear();
    }

    /// Download entire repository (non-GGUF models)
    pub async fn confirm_repository_download(&mut self) {
        self.confirm_scoped_repository_download(RepoScope::WholeModel)
            .await;
    }

    /// Queue a download for a Standard-mode tree selection: a single file
    /// (`path`, `is_dir == false`) or every file under a directory
    /// (`path`, `is_dir == true`). Issue #25 P5: non-GGUF repos previously
    /// only offered whole-repo download.
    async fn confirm_tree_download(&mut self, path: &str, is_dir: bool) {
        self.confirm_scoped_repository_download(RepoScope::Tree { path, is_dir })
            .await;
    }

    /// The whole pipeline behind both non-GGUF confirm flows — gather the
    /// repo files the scope selects, root them under base/author/model,
    /// queue them through the repository-flavor enqueue transaction, and
    /// run the shared post-enqueue tail. Only the scope differs: which
    /// sibling files are picked and how the empty-selection and success
    /// lines are worded.
    async fn confirm_scoped_repository_download(&mut self, scope: RepoScope<'_>) {
        let models = self.models.read().clone();
        let metadata = self.model_metadata.read().clone();

        let model_selected = self.list_state.selected();

        let Some(model_idx) = model_selected else {
            return;
        };
        let Some(meta) = metadata else {
            return;
        };
        let Some(model) = models.get(model_idx) else {
            return;
        };

        let base_path = self.download_path_input.value().to_string();

        // Select the files the scope covers; skip likely directories (no
        // size or trailing '/'). Each file keeps its subdirectory layout.
        let files_to_download: Vec<_> = meta
            .siblings
            .iter()
            .filter(|f| f.size.is_some() && !f.rfilename.ends_with('/') && scope.selects(f))
            .collect();

        if files_to_download.is_empty() {
            match scope {
                RepoScope::WholeModel => {
                    *self.error.write() =
                        Some("No files to download in this repository".to_string());
                }
                RepoScope::Tree { path, .. } => {
                    *self.error.write() = Some(format!("No downloadable files match {}", path));
                    *self.status.write() = "Download cancelled".to_string();
                }
            }
            return;
        }

        let model_root = model_root(&base_path, &model.id);

        // Queue payload; the registry entries are derived from these same
        // fields by the enqueue transaction below.
        let queued: Vec<QueuedDownload> = files_to_download
            .iter()
            .map(|file| QueuedDownload {
                model_id: model.id.clone(),
                revision: crate::api::DEFAULT_REVISION.to_string(),
                filename: file.rfilename.clone(),
                base_path: model_root.clone(),
                expected_sha256: file.lfs.as_ref().map(|lfs| lfs.oid.clone()),
                hf_token: self.options.hf_token.clone(),
                total_size: file.size.unwrap_or(0),
                remote_path: None,
            })
            .collect();

        // Shared enqueue transaction, repository flavor: registry mirror
        // upsert with queued-size entries, queue accounted before the
        // sends, HUD mirror per successful send, failed-send tail rollback.
        // Files whose path fails validation are skipped from the registry
        // AND never queued (audit fix 1a).
        let outcome = self
            .engine
            .enqueue(
                &self.download_tx,
                &queued,
                &EnqueuePolicy::tui_repository(&base_path),
            )
            .await;

        // Per-flow strings for the shared tail: the whole-repo line always
        // says "files" and names the model id; the tree line spells the
        // singular for a single file and names the selected path.
        let success = match scope {
            RepoScope::WholeModel => format!(
                "Queued {} files from {} to {}",
                outcome.sent,
                model.id,
                model_root.display()
            ),
            RepoScope::Tree { path, .. } => format!(
                "Queued {} file{} from {} to {}",
                outcome.sent,
                if outcome.sent == 1 { "" } else { "s" },
                path,
                model_root.display()
            ),
        };
        self.finish_enqueue(&outcome, success, "Failed to start downloads");
    }

    /// Enter in the FLARE revision-picker popup (plans/flare-models.md
    /// plans/flare-models.md §3.5): re-resolve the FULL revision from
    /// `flare_info` by the
    /// choice's commit pin and enqueue through
    /// [`App::trigger_flare_download`]. The popup is already closed when
    /// this runs (the key handler takes its state out first).
    pub async fn confirm_flare_revision_download(
        &mut self,
        model_id: String,
        choices: Vec<FlareRevisionChoice>,
        cursor: usize,
    ) {
        // Binding guard: the Results selection must still be the model
        // whose scan produced the popup (selection changes close the
        // popup via `clear_model_details`, so a mismatch here is
        // defensive only).
        let selected = {
            let models = self.models.read();
            self.list_state
                .selected()
                .and_then(|idx| models.get(idx))
                .map(|model| model.id.clone())
        };
        if selected.as_deref() != Some(model_id.as_str()) {
            *self.error.write() =
                Some("Selection changed — reopen the FLARE revision picker".to_string());
            return;
        }
        let Some(choice) = choices.get(cursor) else {
            return;
        };
        let Some(info) = self.flare_info.read().clone() else {
            *self.error.write() = Some("FLARE scan lost — reselect the model".to_string());
            return;
        };
        let Some(revision) = info
            .revisions
            .iter()
            .find(|revision| revision.target_commit == choice.target_commit)
        else {
            *self.error.write() = Some(format!(
                "Revision {} vanished from the scan — reselect the model",
                choice.revision
            ));
            return;
        };
        self.trigger_flare_download(&model_id, revision).await;
    }

    /// Queue one FLARE revision's package, COMMIT-PINNED
    /// (plans/flare-models.md §3.5):
    ///
    /// - every file's `revision` is the 40-hex `target_commit` — never the
    ///   branch name, so branch movement between scan and download cannot
    ///   mix commits; registry entries store the sha (which round-trips:
    ///   `registry.rs` maps only `"main"` to `None`);
    /// - `expected_sha256` forwards the LFS oid
    ///   (plans/flare-models.md §3.3e) so payloads are
    ///   digest-verified by default;
    /// - the LOCAL filename/destination is `<revision-dir>/<relpath>`
    ///   (plans/flare-models.md §3.3d), so files land at
    ///   `base/author/model/<rev>/<relpath>` and
    ///   cross-revision same-name packages never collide on disk;
    /// - the download set is exactly the package [`FlareRepoInfo`] carries
    ///   (plans/flare-models.md §3.5's "FileSpecs from FlareRepoInfo"):
    ///   packed = the single
    ///   file; directory = `model.safetensors` + `weights/**` +
    ///   `deployments/**` companions.
    ///
    /// Enqueued via the existing `EnqueuePolicy::tui_repository`
    /// transaction (queued-size registry entries, queue accounting, HUD
    /// mirror, rollback — the repository flow's flavor).
    pub async fn trigger_flare_download(&mut self, model_id: &str, revision: &FlareRevision) {
        let base_path = self.options.default_directory.clone();
        let model_root = model_root(&base_path, model_id);
        let files = crate::ui::render::flare_package_files(&revision.package);

        let queued: Vec<QueuedDownload> = files
            .iter()
            .map(|file| {
                let filename = format!("{}/{}", revision.revision, file.rfilename);
                QueuedDownload {
                    model_id: model_id.to_string(),
                    revision: revision.target_commit.clone(),
                    // The plans/flare-models.md §3.3d split: `filename` carries the
                    // revision-dir-prefixed LOCAL destination; `remote_path`
                    // carries the UNPREFIXED repo relpath so every URL
                    // build (enqueue policy, engine, registry) requests
                    // `/resolve/<commit>/<relpath>` while the disk path
                    // keeps the prefix (plans/flare-models.md §3.3d).
                    remote_path: Some(file.rfilename.clone()),
                    filename,
                    base_path: model_root.clone(),
                    expected_sha256: file.lfs.as_ref().map(|lfs| lfs.oid.clone()),
                    hf_token: self.options.hf_token.clone(),
                    total_size: file.size.unwrap_or(0),
                }
            })
            .collect();

        let outcome = self
            .engine
            .enqueue(
                &self.download_tx,
                &queued,
                &EnqueuePolicy::tui_repository(&base_path),
            )
            .await;

        let success = format!(
            "Queued {} FLARE file(s) of {} @{} to {}",
            files.len(),
            revision.revision,
            revision
                .target_commit
                .get(..8)
                .unwrap_or(&revision.target_commit),
            model_root.display()
        );
        self.finish_enqueue(&outcome, success, "Failed to start FLARE download");
    }
}

/// Which repo files a non-GGUF confirm flow downloads: the entire
/// repository, or one Standard-mode tree-pane selection (a single file or
/// every file under a directory). Everything else in the flow — rooting,
/// queue payload, enqueue transaction, post-enqueue strings — is shared.
#[derive(Debug, Clone, Copy)]
enum RepoScope<'a> {
    WholeModel,
    Tree { path: &'a str, is_dir: bool },
}
impl RepoScope<'_> {
    /// Whether this scope selects the given sibling file. Callers pre-filter
    /// directory markers; this adds the scope-specific predicate.
    fn selects(&self, file: &RepoFile) -> bool {
        match self {
            RepoScope::WholeModel => true,
            RepoScope::Tree { path, is_dir } => {
                let prefix = format!("{}/", path.trim_end_matches('/'));
                if *is_dir {
                    file.rfilename.starts_with(&prefix)
                } else {
                    file.rfilename == *path
                }
            }
        }
    }
}

/// The directory every file of `model_id` lands under:
/// `base/author/model_name` for a standard two-part `author/name` id; any
/// other shape falls back to `base` itself.
fn model_root(base: &str, model_id: &str) -> PathBuf {
    model_root_or(base, model_id, PathBuf::from(base))
}

/// [`model_root`] with a caller-chosen fallback for malformed model ids
/// (the resume flow falls back to the recorded local_path's parent).
fn model_root_or(base: &str, model_id: &str, fallback: PathBuf) -> PathBuf {
    let parts: Vec<&str> = model_id.split('/').collect();
    match parts[..] {
        [author, name] => PathBuf::from(base).join(author).join(name),
        _ => fallback,
    }
}

/// plans/flare-models.md §3.3d TUI resume shape test: does this registry
/// entry carry a revision-scoped FLARE destination (the form
/// [`App::trigger_flare_download`] records)? The persisted
/// `remote_path` field is the marker — every FLARE frontend (CLI and TUI)
/// sets it at enqueue, and no legacy flow does.
fn is_flare_shaped_entry(metadata: &DownloadMetadata) -> bool {
    metadata.remote_path.is_some()
}

/// plans/flare-models.md §3.3d: derive the resume base from the
/// RECORDED destination. The
/// recorded root is `local_path` minus the recorded-filename suffix — the
/// unique base whose `join(filename)` reproduces `local_path` — and it
/// must still exist on disk. A `local_path` that does not end with the
/// recorded filename, or a root directory that is gone, is a hard error
/// (the user restores it or deletes the entry; nothing re-enqueues onto a
/// guessed destination).
fn recorded_flare_root(metadata: &DownloadMetadata) -> Result<PathBuf, String> {
    let local = PathBuf::from(&metadata.local_path);
    let filename_parts: Vec<_> = metadata
        .filename
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let local_parts: Vec<_> = local.components().collect();
    let suffix_len = filename_parts.len();
    let suffix_matches = local_parts.len() > suffix_len
        && local_parts[local_parts.len() - suffix_len..]
            .iter()
            .zip(&filename_parts)
            .all(|(component, name)| component.as_os_str().to_str() == Some(*name));
    if !suffix_matches {
        return Err(format!(
            "recorded path {} does not end with the recorded filename {} — delete the registry entry or fix its local_path",
            local.display(),
            metadata.filename
        ));
    }
    let root: PathBuf = local_parts[..local_parts.len() - suffix_len]
        .iter()
        .collect();
    if !root.is_dir() {
        return Err(format!(
            "recorded directory {} is missing — restore it or delete the registry entry",
            root.display()
        ));
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    //! W4.4 characterization tests for the three `confirm_*` download
    //! flows. They pin each flow's OBSERVABLE effects — status/error
    //! strings, queue accounting, HUD items mirror, registry entries,
    //! download-channel messages, and the popup clear driven through the
    //! real `on_key_event` dispatch — so the unification refactor must keep
    //! every one of them byte-for-byte. App-level fixtures seed the same
    //! state the UI path would (`models`/`quantizations`/`model_metadata`
    //! plus selection state); `App::new` is sync and lazy (EventStream is
    //! never polled here), so the TUI container is directly
    //! test-constructible.
    //!
    //! Env discipline mirrors `engine.rs`'s tests: HOME (config + registry
    //! path) and HF_ENDPOINT are redirected under the crate-wide `ENV_MUTEX`.
    use super::*;
    use crate::models::{
        DownloadStatus, FlareConfigFacts, FlarePackage, FlareRepoInfo, FlareRevision, LfsInfo,
        ModelDisplayMode, ModelInfo, ModelMetadata, ParsedPackageTokens, PopupMode,
        QuantizationGroup, QuantizationInfo, RefKind, RepoFile,
    };

    /// Find a guaranteed-closed localhost port (bind then drop the listener).
    fn closed_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    struct EnvGuard {
        data_dir: Option<std::ffi::OsString>,
        endpoint: Option<String>,
    }

    impl EnvGuard {
        fn install(data_dir: &std::path::Path, endpoint: &str) -> Self {
            // `RUST_HF_DOWNLOADER_DATA_DIR` moves the registry path on every
            // platform; `HOME` alone would not isolate on Windows, where
            // `dirs::home_dir()` reads `USERPROFILE` instead.
            let guard = Self {
                data_dir: std::env::var_os(crate::paths::ENV_DATA_DIR),
                endpoint: std::env::var("HF_ENDPOINT").ok(),
            };
            std::env::set_var(crate::paths::ENV_DATA_DIR, data_dir);
            std::env::set_var("HF_ENDPOINT", endpoint);
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.data_dir {
                Some(d) => std::env::set_var(crate::paths::ENV_DATA_DIR, d),
                None => std::env::remove_var(crate::paths::ENV_DATA_DIR),
            }
            match &self.endpoint {
                Some(e) => std::env::set_var("HF_ENDPOINT", e),
                None => std::env::remove_var("HF_ENDPOINT"),
            }
        }
    }

    fn test_model() -> ModelInfo {
        ModelInfo {
            id: "author/model".to_string(),
            author: Some("author".to_string()),
            downloads: 0,
            likes: 0,
            tags: Vec::new(),
            last_modified: None,
        }
    }

    /// One GGUF quant group; `files` are (filename, size, sha256) rows.
    fn test_quant_group(files: &[(&str, u64, &str)]) -> QuantizationGroup {
        QuantizationGroup {
            quant_type: "Q4_K_M".to_string(),
            files: files
                .iter()
                .map(|(filename, size, sha)| QuantizationInfo {
                    quant_type: "Q4_K_M".to_string(),
                    filename: filename.to_string(),
                    size: *size,
                    sha256: Some(sha.to_string()),
                })
                .collect(),
            total_size: files.iter().map(|(_, size, _)| size).sum(),
        }
    }

    fn test_repo_file(rfilename: &str, size: Option<u64>, lfs_oid: Option<&str>) -> RepoFile {
        RepoFile {
            rfilename: rfilename.to_string(),
            size,
            oid: None,
            lfs: lfs_oid.map(|oid| LfsInfo {
                oid: oid.to_string(),
                size: size.unwrap_or(0),
                pointer_size: 0,
            }),
        }
    }

    /// Fresh App with model list + selection seeded, popup open, download
    /// base path pointed at `<tmp>/dl`, no token. Callers seed the
    /// flow-specific state (quants / metadata / pending tree selection).
    fn app_with_model_selected(tmp: &std::path::Path) -> App {
        let mut app = App::new();
        *app.models.write() = vec![test_model()];
        app.list_state.select(Some(0));
        app.options.hf_token = None;
        app.download_path_input =
            tui_input::Input::default().with_value(tmp.join("dl").to_string_lossy().to_string());
        app.popup_mode = PopupMode::DownloadPath;
        app
    }

    /// The download base path every fixture uses (`<tmp>/dl`).
    fn base_path(tmp: &std::path::Path) -> String {
        tmp.join("dl").to_string_lossy().to_string()
    }

    /// Drain every message currently buffered in the download channel.
    async fn drain_downloads(app: &App) -> Vec<QueuedDownload> {
        let mut rx = app.engine.download_rx.lock().await;
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    /// Queue accounting + HUD summaries for a successful confirm.
    async fn assert_queue_accounting(app: &App, count: usize, bytes: u64, names: &[&str]) {
        let queue = app.engine.queue.download_queue_totals.lock().await;
        assert_eq!(queue.size, count, "queue size");
        assert_eq!(queue.bytes, bytes, "queue bytes");
        drop(queue);
        let items = app.engine.queue.download_queue_items.lock().await;
        let got: Vec<&str> = items.iter().map(|i| i.filename.as_str()).collect();
        assert_eq!(got, names, "HUD queue summaries in send order");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn confirm_single_quant_file_queues_one_part_and_clears_popup() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-confirm-quant-single-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let filename = "author/model-Q4_K_M.gguf";
        let mut app = app_with_model_selected(&tmp);
        *app.quantizations.write() = vec![test_quant_group(&[(filename, 10, "sha-a")])];
        app.quant_list_state.select(Some(0));
        app.quant_file_list_state.select(Some(0));
        app.focused_pane = FocusedPane::QuantizationFiles;

        // Enter in the DownloadPath popup: the real key path users take.
        app.on_key_event(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Enter,
        ))
        .await;

        // Popup closed by the key handler; single-file status string.
        assert_eq!(app.popup_mode, PopupMode::None);
        let root = PathBuf::from(base_path(&tmp)).join("author").join("model");
        assert_eq!(
            *app.status.read(),
            format!("Starting download of {} to {}", filename, root.display())
        );
        assert!(app.error.read().is_none());

        // One file accounted and one channel message with every field.
        assert_queue_accounting(&app, 1, 10, &[filename]).await;
        let drained = drain_downloads(&app).await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].model_id, "author/model");
        assert_eq!(drained[0].revision, crate::api::DEFAULT_REVISION);
        assert_eq!(drained[0].filename, filename);
        assert_eq!(drained[0].base_path, root);
        assert_eq!(drained[0].expected_sha256.as_deref(), Some("sha-a"));
        assert_eq!(drained[0].hf_token, None);
        assert_eq!(drained[0].total_size, 10);

        // GGUF quant flavor: zero-size registry entry, no revision.
        let registry = app.engine.download_registry.lock().await;
        assert_eq!(registry.downloads.len(), 1);
        assert_eq!(registry.downloads[0].total_size, 0);
        assert_eq!(registry.downloads[0].status, DownloadStatus::Incomplete);
        assert_eq!(registry.downloads[0].revision, None);
        assert_eq!(
            registry.downloads[0].expected_sha256.as_deref(),
            Some("sha-a")
        );
        assert_eq!(
            registry.downloads[0].url,
            crate::api::resolve_url("author/model", filename, crate::api::DEFAULT_REVISION)
        );
        assert_eq!(
            PathBuf::from(&registry.downloads[0].local_path),
            root.join(filename)
        );
        drop(registry);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn confirm_quant_group_queues_all_parts_with_multi_part_status() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-confirm-quant-group-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let f1 = "author/model-Q4_K_M-00001-of-00002.gguf";
        let f2 = "author/model-Q4_K_M-00002-of-00002.gguf";
        let mut app = app_with_model_selected(&tmp);
        *app.quantizations.write() =
            vec![test_quant_group(&[(f1, 10, "sha-1"), (f2, 20, "sha-2")])];
        app.quant_list_state.select(Some(0));
        app.focused_pane = FocusedPane::QuantizationGroups;

        app.on_key_event(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Enter,
        ))
        .await;

        // Group focus downloads every part; the multi-part status string
        // names the FIRST file.
        assert_eq!(app.popup_mode, PopupMode::None);
        let root = PathBuf::from(base_path(&tmp)).join("author").join("model");
        assert_eq!(
            *app.status.read(),
            format!("Queued 2 parts of {} to {}", f1, root.display())
        );
        assert!(app.error.read().is_none());

        assert_queue_accounting(&app, 2, 30, &[f1, f2]).await;
        let drained = drain_downloads(&app).await;
        assert_eq!(drained.len(), 2);
        for (msg, (filename, size, sha)) in drained
            .iter()
            .zip([(f1, 10u64, "sha-1"), (f2, 20, "sha-2")])
        {
            assert_eq!(msg.filename, filename);
            assert_eq!(msg.base_path, root);
            assert_eq!(msg.total_size, size);
            assert_eq!(msg.expected_sha256.as_deref(), Some(sha));
        }

        let registry = app.engine.download_registry.lock().await;
        assert_eq!(registry.downloads.len(), 2);
        assert!(registry.downloads.iter().all(|d| d.total_size == 0));
        drop(registry);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn confirm_repository_download_queues_files_only_under_model_root() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("app-confirm-repo-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let mut app = app_with_model_selected(&tmp);
        *app.display_mode.write() = ModelDisplayMode::Standard;
        app.focused_pane = FocusedPane::Models;
        *app.model_metadata.write() = Some(ModelMetadata {
            model_id: "author/model".to_string(),
            library_name: None,
            pipeline_tag: None,
            card_data: None,
            siblings: vec![
                test_repo_file("README.md", Some(100), None),
                test_repo_file("sub/model.bin", Some(200), Some("lfs-sha")),
                // Directory markers must be filtered out: no size, or a
                // trailing slash.
                test_repo_file("empty-dir/", None, None),
                test_repo_file("dir/", Some(1), None),
            ],
            tags: Vec::new(),
            sha: None,
        });

        app.on_key_event(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Enter,
        ))
        .await;

        assert_eq!(app.popup_mode, PopupMode::None);
        let root = PathBuf::from(base_path(&tmp)).join("author").join("model");
        assert_eq!(
            *app.status.read(),
            format!("Queued 2 files from author/model to {}", root.display())
        );
        assert!(app.error.read().is_none());

        assert_queue_accounting(&app, 2, 300, &["README.md", "sub/model.bin"]).await;
        let drained = drain_downloads(&app).await;
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].filename, "README.md");
        assert_eq!(drained[0].base_path, root);
        assert_eq!(drained[0].expected_sha256, None);
        assert_eq!(drained[0].total_size, 100);
        assert_eq!(drained[1].filename, "sub/model.bin");
        assert_eq!(drained[1].base_path, root);
        assert_eq!(drained[1].expected_sha256.as_deref(), Some("lfs-sha"));
        assert_eq!(drained[1].total_size, 200);

        // Repository flavor records the QUEUED size and keeps each file's
        // subdirectory under base/author/model.
        let registry = app.engine.download_registry.lock().await;
        assert_eq!(registry.downloads.len(), 2);
        assert_eq!(registry.downloads[0].total_size, 100);
        assert_eq!(registry.downloads[1].total_size, 200);
        assert_eq!(
            PathBuf::from(&registry.downloads[1].local_path),
            root.join("sub").join("model.bin")
        );
        assert_eq!(
            registry.downloads[1].expected_sha256.as_deref(),
            Some("lfs-sha")
        );
        drop(registry);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Audit fix 1a pin (all-invalid batch): an invalid file is now
    /// reported AND dropped, so a batch whose every file is invalid sends
    /// nothing. The `sent == 0` tail of `finish_enqueue` must then keep the
    /// last per-file `PathError` visible instead of overwriting it with the
    /// generic failure line, and nothing may be queued, mirrored, or sent.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn confirm_repository_with_only_invalid_files_keeps_the_path_error() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-confirm-all-invalid-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let mut app = app_with_model_selected(&tmp);
        let status_before = app.status.read().clone();
        *app.display_mode.write() = ModelDisplayMode::Standard;
        app.focused_pane = FocusedPane::Models;
        *app.model_metadata.write() = Some(ModelMetadata {
            model_id: "author/model".to_string(),
            library_name: None,
            pipeline_tag: None,
            card_data: None,
            siblings: vec![
                // `..` component and a Windows-reserved device stem.
                test_repo_file("../evil.bin", Some(10), None),
                test_repo_file("con.gguf", Some(20), None),
            ],
            tags: Vec::new(),
            sha: None,
        });

        app.on_key_event(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Enter,
        ))
        .await;

        assert_eq!(app.popup_mode, PopupMode::None);
        // The LAST invalid file's PathError stays visible — the generic
        // "Failed to start downloads" line must not replace it, and the
        // status line is untouched.
        assert_eq!(
            app.error.read().as_deref(),
            Some("Invalid filename 'con.gguf': Invalid filename component: con.gguf")
        );
        assert_eq!(*app.status.read(), status_before);

        // Nothing queued, mirrored, or sent; the registry stayed empty.
        assert_queue_accounting(&app, 0, 0, &[]).await;
        assert!(drain_downloads(&app).await.is_empty());
        assert!(app
            .engine
            .download_registry
            .lock()
            .await
            .downloads
            .is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn confirm_tree_directory_download_queues_subtree_only() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("app-confirm-tree-dir-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let mut app = app_with_model_selected(&tmp);
        *app.model_metadata.write() = Some(ModelMetadata {
            model_id: "author/model".to_string(),
            library_name: None,
            pipeline_tag: None,
            card_data: None,
            siblings: vec![
                test_repo_file("sub/a.bin", Some(10), Some("sha-a")),
                test_repo_file("sub/b.bin", Some(20), Some("sha-b")),
                test_repo_file("other.bin", Some(30), None),
            ],
            tags: Vec::new(),
            sha: None,
        });
        app.pending_tree_download = Some(("sub".to_string(), true));

        app.on_key_event(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Enter,
        ))
        .await;

        assert_eq!(app.popup_mode, PopupMode::None);
        let root = PathBuf::from(base_path(&tmp)).join("author").join("model");
        assert_eq!(
            *app.status.read(),
            format!("Queued 2 files from sub to {}", root.display())
        );
        assert!(app.error.read().is_none());

        // Only the subtree — other.bin stays out of every mirror.
        assert_queue_accounting(&app, 2, 30, &["sub/a.bin", "sub/b.bin"]).await;
        let drained = drain_downloads(&app).await;
        assert_eq!(drained.len(), 2);
        assert!(drained.iter().all(|m| m.base_path == root));

        let registry = app.engine.download_registry.lock().await;
        assert_eq!(registry.downloads.len(), 2);
        assert_eq!(
            PathBuf::from(&registry.downloads[0].local_path),
            root.join("sub").join("a.bin")
        );
        drop(registry);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn confirm_tree_file_download_queues_exact_file_with_singular_status() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-confirm-tree-file-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let mut app = app_with_model_selected(&tmp);
        *app.model_metadata.write() = Some(ModelMetadata {
            model_id: "author/model".to_string(),
            library_name: None,
            pipeline_tag: None,
            card_data: None,
            siblings: vec![
                test_repo_file("config.json", Some(5), None),
                test_repo_file("sub/a.bin", Some(10), None),
            ],
            tags: Vec::new(),
            sha: None,
        });
        app.pending_tree_download = Some(("config.json".to_string(), false));

        app.on_key_event(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Enter,
        ))
        .await;

        // Singular "file" — the one-file spelling of the tree status line.
        assert_eq!(app.popup_mode, PopupMode::None);
        let root = PathBuf::from(base_path(&tmp)).join("author").join("model");
        assert_eq!(
            *app.status.read(),
            format!("Queued 1 file from config.json to {}", root.display())
        );
        assert!(app.error.read().is_none());

        assert_queue_accounting(&app, 1, 5, &["config.json"]).await;
        let drained = drain_downloads(&app).await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].filename, "config.json");
        assert_eq!(drained[0].base_path, root);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn confirm_tree_download_without_matches_cancels_and_queues_nothing() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-confirm-tree-miss-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let mut app = app_with_model_selected(&tmp);
        *app.model_metadata.write() = Some(ModelMetadata {
            model_id: "author/model".to_string(),
            library_name: None,
            pipeline_tag: None,
            card_data: None,
            siblings: vec![test_repo_file("sub/a.bin", Some(10), None)],
            tags: Vec::new(),
            sha: None,
        });
        app.pending_tree_download = Some(("nope".to_string(), true));

        app.on_key_event(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Enter,
        ))
        .await;

        // Cancellation path: both strings, and nothing enqueued anywhere.
        assert_eq!(*app.status.read(), "Download cancelled");
        assert_eq!(
            app.error.read().as_deref(),
            Some("No downloadable files match nope")
        );
        assert!(
            app.pending_tree_download.is_none(),
            "pending selection consumed"
        );
        assert_queue_accounting(&app, 0, 0, &[]).await;
        assert!(drain_downloads(&app).await.is_empty());
        assert!(app
            .engine
            .download_registry
            .lock()
            .await
            .downloads
            .is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resume_incomplete_downloads_requeues_under_model_root_or_recorded_parent() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-resume-incomplete-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let default_dir = tmp.join("dl");
        let mut app = App::new();
        app.options.hf_token = None;
        app.options.default_directory = default_dir.to_string_lossy().to_string();
        app.incomplete_downloads = vec![
            // Two-part model id → default_dir/author/model.
            crate::models::DownloadMetadata {
                model_id: "author/model".to_string(),
                filename: "f.gguf".to_string(),
                url: "https://example.invalid/a".to_string(),
                local_path: "/elsewhere/f.gguf".to_string(),
                total_size: 10,
                downloaded_size: 0,
                status: DownloadStatus::Incomplete,
                expected_sha256: Some("sha".to_string()),
                revision: None,
                remote_path: None,
            },
            // Malformed model id → the recorded local_path's parent.
            crate::models::DownloadMetadata {
                model_id: "not-a-model-id".to_string(),
                filename: "g.gguf".to_string(),
                url: "https://example.invalid/b".to_string(),
                local_path: "/recorded/dir/g.gguf".to_string(),
                total_size: 20,
                downloaded_size: 5,
                status: DownloadStatus::Incomplete,
                expected_sha256: None,
                revision: Some("rev-x".to_string()),
                remote_path: None,
            },
        ];

        app.resume_incomplete_downloads().await;

        assert_eq!(*app.status.read(), "Resuming 2 incomplete download(s)");
        assert!(app.incomplete_downloads.is_empty());

        let drained = drain_downloads(&app).await;
        assert_eq!(drained.len(), 2);
        assert_eq!(
            drained[0].base_path,
            default_dir.join("author").join("model")
        );
        assert_eq!(drained[0].revision, crate::api::DEFAULT_REVISION);
        assert_eq!(drained[1].base_path, PathBuf::from("/recorded/dir"));
        assert_eq!(drained[1].revision, "rev-x");

        // Resume flavor: queue accounted AFTER the sends (all sent), no
        // registry writes (mirror stays empty).
        assert_queue_accounting(&app, 2, 30, &["f.gguf", "g.gguf"]).await;
        assert!(app
            .engine
            .download_registry
            .lock()
            .await
            .downloads
            .is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn confirm_quant_flow_with_no_file_selected_reports_error_and_queues_nothing() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-confirm-quant-empty-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let mut app = app_with_model_selected(&tmp);
        *app.quantizations.write() = vec![test_quant_group(&[("f.gguf", 10, "sha")])];
        app.quant_list_state.select(Some(0));
        // Files pane focused but nothing selected → empty selection guard.
        app.focused_pane = FocusedPane::QuantizationFiles;

        app.on_key_event(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Enter,
        ))
        .await;

        assert_eq!(
            app.error.read().as_deref(),
            Some("No files selected for download")
        );
        assert_queue_accounting(&app, 0, 0, &[]).await;
        assert!(drain_downloads(&app).await.is_empty());
        assert!(app
            .engine
            .download_registry
            .lock()
            .await
            .downloads
            .is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// M1 pin for the TUI delete flow (typed-op conversion, validation-first):
    /// the registry effect of deleting incomplete downloads — the selected
    /// urls drop from DISK (exact TOML bytes), the engine mirror ends up
    /// equal to what is on disk, the status line reports the deleted count,
    /// and the `.incomplete` file on disk is removed. Disk and mirror are
    /// seeded identically (what the TUI startup scan produces); the delete
    /// file-removal step runs against a real temp file.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn delete_incomplete_pins_disk_bytes_mirror_and_status() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-delete-incomplete-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        fn entry(url: &str, filename: &str, local_path: String) -> DownloadMetadata {
            DownloadMetadata {
                model_id: "org/model".to_string(),
                filename: filename.to_string(),
                url: url.to_string(),
                local_path,
                total_size: 9,
                downloaded_size: 4,
                status: DownloadStatus::Incomplete,
                expected_sha256: None,
                revision: None,
                remote_path: None,
            }
        }

        let selected = entry(
            "https://huggingface.co/org/model/resolve/main/selected.bin",
            "selected.bin",
            tmp.join("selected.bin").to_string_lossy().to_string(),
        );
        let other = entry(
            "https://huggingface.co/org/model/resolve/main/other.bin",
            "other.bin",
            tmp.join("other.bin").to_string_lossy().to_string(),
        );

        // Disk seeded through the typed op, mirror seeded from disk — the
        // state the TUI startup scan leaves behind.
        crate::registry::upsert_pending(&[selected.clone(), other.clone()]);
        let mut app = App::new();
        *app.engine.download_registry.lock().await = crate::registry::read_registry();
        app.incomplete_downloads = vec![selected.clone()];

        // The `.incomplete` file the delete flow removes on disk, plus
        // its resume journal (issue #50): both must go.
        let incomplete_file =
            std::path::PathBuf::from(format!("{}.incomplete", selected.local_path));
        std::fs::write(&incomplete_file, b"partial").expect("write .incomplete fixture");
        let journal_file =
            std::path::PathBuf::from(format!("{}.state.json", incomplete_file.display()));
        std::fs::write(&journal_file, b"{}").expect("write journal fixture");

        app.delete_incomplete_downloads().await;

        // Status line: one file removed, no errors.
        assert_eq!(*app.status.read(), "Deleted 1 incomplete file(s)");
        assert!(app.incomplete_downloads.is_empty());
        assert!(!incomplete_file.exists(), ".incomplete file removed");
        assert!(!journal_file.exists(), "resume journal removed");

        // Disk: exact TOML golden — only the unselected entry survives.
        let disk_bytes =
            std::fs::read_to_string(crate::paths::registry_path()).expect("registry readable");
        let expected = toml::to_string_pretty(&crate::models::DownloadRegistry {
            downloads: vec![other.clone()],
        })
        .expect("serialize expected registry");
        assert_eq!(disk_bytes, expected);

        // Mirror: replaced from the post-delete registry state — equal to
        // what is on disk.
        let mirror = app.engine.download_registry.lock().await;
        assert_eq!(mirror.downloads.len(), 1);
        assert_eq!(mirror.downloads[0].url, other.url);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ------------------------------------------------------------------
    // plans/flare-models.md §3.5: the FLARE download flow, and §3.3d's
    // TUI resume rule for FLARE-shaped registry entries.
    // ------------------------------------------------------------------

    const FLARE_COMMIT: &str = "1f0a2b3c4d5e6f708192a3b4c5d6e7f809182a3b";
    const FLARE_COMMIT_2: &str = "0e1d2c3b4a5968778695a4b3c2d1e0f9988776655";

    fn packed_flare_revision() -> FlareRevision {
        FlareRevision {
            revision: "gb202".to_string(),
            aliases: vec!["v3".to_string()],
            kind: RefKind::Branch,
            target_commit: FLARE_COMMIT.to_string(),
            package: FlarePackage::Packed {
                file: test_repo_file("pkg.safetensors", Some(111), Some("lfs-sha")),
            },
            parsed: ParsedPackageTokens {
                tp: Some(2),
                arch: Some("gb202".to_string()),
                ..ParsedPackageTokens::default()
            },
        }
    }

    fn directory_flare_revision() -> FlareRevision {
        FlareRevision {
            revision: "v2".to_string(),
            aliases: Vec::new(),
            kind: RefKind::Tag,
            target_commit: FLARE_COMMIT_2.to_string(),
            package: FlarePackage::Directory {
                primary: test_repo_file("model.safetensors", Some(1_000), Some("dir-sha-1")),
                companions: vec![
                    test_repo_file("weights/0.safetensors", Some(2_000), Some("dir-sha-2")),
                    test_repo_file("deployments/gb202-tp1/rank-0/plan.json", Some(10), None),
                ],
            },
            parsed: ParsedPackageTokens::default(),
        }
    }

    fn flare_repo(revisions: Vec<FlareRevision>) -> FlareRepoInfo {
        FlareRepoInfo {
            revisions,
            config: FlareConfigFacts::default(),
            publisher: None,
            dropped_refs: Vec::new(),
        }
    }

    /// Fresh App in Flare mode with the model selected, `flare_info`
    /// seeded (packed gb202 + directory v2), FlareRevisions focused, and
    /// the download default directory pointed at `<tmp>/dl`.
    fn app_in_flare_mode(tmp: &std::path::Path) -> App {
        let mut app = App::new();
        *app.models.write() = vec![test_model()];
        app.list_state.select(Some(0));
        app.options.hf_token = None;
        app.options.default_directory = tmp.join("dl").to_string_lossy().to_string();
        *app.display_mode.write() = ModelDisplayMode::Flare;
        app.focused_pane = FocusedPane::FlareRevisions;
        app.flare_list_state.select(Some(0));
        *app.flare_info.write() = Some(flare_repo(vec![
            packed_flare_revision(),
            directory_flare_revision(),
        ]));
        app.popup_mode = PopupMode::None;
        app
    }

    fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::from(code)
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn trigger_flare_download_pins_commit_and_prefixes_destinations() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("app-flare-trigger-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let mut app = app_in_flare_mode(&tmp);
        let revision = packed_flare_revision();
        app.trigger_flare_download("author/model", &revision).await;

        let root = tmp.join("dl").join("author").join("model");
        assert_eq!(
            *app.status.read(),
            format!(
                "Queued 1 FLARE file(s) of gb202 @1f0a2b3c to {}",
                root.display()
            )
        );
        assert!(app.error.read().is_none());

        // The commit is the pin: the queue message and the registry entry
        // both carry the 40-hex sha, never the branch name.
        assert_queue_accounting(&app, 1, 111, &["gb202/pkg.safetensors"]).await;
        let drained = drain_downloads(&app).await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].model_id, "author/model");
        assert_eq!(drained[0].revision, FLARE_COMMIT);
        assert_eq!(drained[0].filename, "gb202/pkg.safetensors");
        assert_eq!(drained[0].base_path, root);
        assert_eq!(drained[0].expected_sha256.as_deref(), Some("lfs-sha"));
        assert_eq!(drained[0].total_size, 111);

        // tui_repository flavor: queued-size mirror entries, revision =
        // the commit sha (round-trips: only "main" maps to None), the
        // revision-dir-prefixed local destination, and the LFS oid
        // forwarded for digest verification.
        let registry = app.engine.download_registry.lock().await;
        assert_eq!(registry.downloads.len(), 1);
        assert_eq!(registry.downloads[0].total_size, 111);
        assert_eq!(
            registry.downloads[0].revision.as_deref(),
            Some(FLARE_COMMIT)
        );
        assert_eq!(
            registry.downloads[0].expected_sha256.as_deref(),
            Some("lfs-sha")
        );
        assert_eq!(
            PathBuf::from(&registry.downloads[0].local_path),
            root.join("gb202").join("pkg.safetensors")
        );
        // The plans/flare-models.md §3.3d split rides the transport: the recorded URL is the
        // UNPREFIXED repo-relpath at the commit pin — the disk path keeps
        // the `gb202/` prefix, the request URL does not.
        assert_eq!(
            registry.downloads[0].url,
            crate::api::resolve_url("author/model", "pkg.safetensors", FLARE_COMMIT)
        );
        assert_eq!(
            registry.downloads[0].remote_path.as_deref(),
            Some("pkg.safetensors")
        );
        drop(registry);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn trigger_flare_download_directory_package_queues_primary_and_companions() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-flare-trigger-dir-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let mut app = app_in_flare_mode(&tmp);
        let revision = directory_flare_revision();
        app.trigger_flare_download("author/model", &revision).await;

        // Directory form: primary + weights/** + deployments/** — every
        // filename prefixed with the revision dir, nested repo layout
        // preserved under it. plan.json carries no LFS oid ⇒ no sha.
        assert_queue_accounting(
            &app,
            3,
            3_010,
            &[
                "v2/model.safetensors",
                "v2/weights/0.safetensors",
                "v2/deployments/gb202-tp1/rank-0/plan.json",
            ],
        )
        .await;
        let drained = drain_downloads(&app).await;
        assert!(drained.iter().all(|m| m.revision == FLARE_COMMIT_2));
        assert_eq!(drained[0].expected_sha256.as_deref(), Some("dir-sha-1"));
        assert_eq!(drained[1].expected_sha256.as_deref(), Some("dir-sha-2"));
        assert_eq!(drained[2].expected_sha256, None);

        let registry = app.engine.download_registry.lock().await;
        assert_eq!(
            PathBuf::from(&registry.downloads[2].local_path),
            tmp.join("dl")
                .join("author")
                .join("model")
                .join("v2")
                .join("deployments")
                .join("gb202-tp1")
                .join("rank-0")
                .join("plan.json")
        );
        drop(registry);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 'd' in Flare mode opens the revision-picker popup
    /// (plans/flare-models.md §3.5): snapshot
    /// choices bound to the selected model; Esc cancels without enqueue.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn d_in_flare_mode_opens_revision_popup_and_esc_cancels() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("app-flare-popup-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let mut app = app_in_flare_mode(&tmp);

        app.on_key_event(key(crossterm::event::KeyCode::Char('d')))
            .await;

        let PopupMode::FlareRevision {
            model_id,
            choices,
            cursor,
        } = &app.popup_mode
        else {
            panic!(
                "'d' must open the FlareRevision popup, got {:?}",
                app.popup_mode
            );
        };
        assert_eq!(model_id, "author/model");
        assert_eq!(*cursor, 0);
        assert_eq!(choices.len(), 2);
        // The snapshot rows mirror the scan: sizes, file counts, arch
        // tokens (the star input), kind, and the commit pins.
        assert_eq!(choices[0].revision, "gb202");
        assert_eq!(choices[0].aliases, vec!["v3".to_string()]);
        assert_eq!(choices[0].size, 111);
        assert_eq!(choices[0].files, 1);
        assert_eq!(choices[0].arch.as_deref(), Some("gb202"));
        assert_eq!(choices[0].target_commit, FLARE_COMMIT);
        assert_eq!(choices[1].revision, "v2");
        assert_eq!(choices[1].files, 3);
        assert_eq!(choices[1].size, 3_010);
        assert_eq!(
            *app.status.read(),
            "Select a FLARE revision to download (2 available)"
        );

        // Esc: cancel — popup closed, nothing enqueued anywhere.
        app.on_key_event(key(crossterm::event::KeyCode::Esc)).await;
        assert_eq!(app.popup_mode, PopupMode::None);
        assert_eq!(*app.status.read(), "Download cancelled");
        assert_queue_accounting(&app, 0, 0, &[]).await;
        assert!(drain_downloads(&app).await.is_empty());
        assert!(app
            .engine
            .download_registry
            .lock()
            .await
            .downloads
            .is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Popup arrows move the cursor (bounded), Enter enqueues the CHOSEN
    /// revision (re-resolved from flare_info by commit pin).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn flare_popup_arrows_move_and_enter_enqueues_the_chosen_revision() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-flare-popup-enter-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let mut app = app_in_flare_mode(&tmp);
        app.on_key_event(key(crossterm::event::KeyCode::Char('d')))
            .await;

        // Down moves to the directory-form card; Up clamps at 0.
        app.on_key_event(key(crossterm::event::KeyCode::Down)).await;
        app.on_key_event(key(crossterm::event::KeyCode::Down)).await;
        if let PopupMode::FlareRevision { cursor, .. } = &app.popup_mode {
            assert_eq!(*cursor, 1, "cursor bounded at the last row");
        }
        app.on_key_event(key(crossterm::event::KeyCode::Up)).await;
        if let PopupMode::FlareRevision { cursor, .. } = &app.popup_mode {
            assert_eq!(*cursor, 0);
        }
        app.on_key_event(key(crossterm::event::KeyCode::Down)).await;

        app.on_key_event(key(crossterm::event::KeyCode::Enter))
            .await;

        assert_eq!(app.popup_mode, PopupMode::None);
        // The SECOND revision (v2, directory form) — the cursor row at
        // Enter time, pinned by its own commit: all three package files.
        assert_queue_accounting(
            &app,
            3,
            3_010,
            &[
                "v2/model.safetensors",
                "v2/weights/0.safetensors",
                "v2/deployments/gb202-tp1/rank-0/plan.json",
            ],
        )
        .await;
        let drained = drain_downloads(&app).await;
        assert!(drained.iter().all(|m| m.revision == FLARE_COMMIT_2));
        assert!(app.error.read().is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// plans/flare-models.md §3.3d TUI resume rule: a FLARE-shaped entry
    /// resumes at its
    /// RECORDED destination — the base derives from the recorded
    /// local_path, never the live default dir (which differs here).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resume_flare_entry_uses_recorded_root_not_live_default() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("app-resume-flare-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        // The recorded root exists on disk under a DIFFERENT base than the
        // live default dir — the interrupted download's true home.
        let recorded_root = tmp.join("recorded").join("author").join("model");
        std::fs::create_dir_all(&recorded_root).unwrap();

        let mut app = App::new();
        app.options.hf_token = None;
        app.options.default_directory = tmp.join("live").to_string_lossy().to_string();
        app.incomplete_downloads = vec![crate::models::DownloadMetadata {
            model_id: "author/model".to_string(),
            filename: "gb202/pkg.safetensors".to_string(),
            url: "https://example.invalid/a".to_string(),
            local_path: recorded_root
                .join("gb202")
                .join("pkg.safetensors")
                .to_string_lossy()
                .to_string(),
            total_size: 111,
            downloaded_size: 5,
            status: DownloadStatus::Incomplete,
            expected_sha256: Some("lfs-sha".to_string()),
            revision: Some(FLARE_COMMIT.to_string()),
            remote_path: Some("pkg.safetensors".to_string()),
        }];

        app.resume_incomplete_downloads().await;

        assert_eq!(*app.status.read(), "Resuming 1 incomplete download(s)");
        assert!(app.error.read().is_none());
        let drained = drain_downloads(&app).await;
        assert_eq!(drained.len(), 1);
        // The RECORDED root — not <live>/author/model.
        assert_eq!(drained[0].base_path, recorded_root);
        assert_eq!(drained[0].filename, "gb202/pkg.safetensors");
        assert_eq!(drained[0].revision, FLARE_COMMIT);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// plans/flare-models.md §3.3d: a FLARE-shaped entry whose recorded
    /// root is GONE is a hard
    /// error — nothing enqueues (no guessed destination).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resume_flare_entry_with_missing_recorded_root_aborts() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-resume-flare-missing-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        // Recorded root NOT created — the directory was deleted.
        let recorded_root = tmp.join("gone").join("author").join("model");

        let mut app = App::new();
        app.options.hf_token = None;
        app.options.default_directory = tmp.join("live").to_string_lossy().to_string();
        app.incomplete_downloads = vec![crate::models::DownloadMetadata {
            model_id: "author/model".to_string(),
            filename: "gb202/pkg.safetensors".to_string(),
            url: "https://example.invalid/a".to_string(),
            local_path: recorded_root
                .join("gb202")
                .join("pkg.safetensors")
                .to_string_lossy()
                .to_string(),
            total_size: 111,
            downloaded_size: 5,
            status: DownloadStatus::Incomplete,
            expected_sha256: None,
            revision: Some(FLARE_COMMIT.to_string()),
            remote_path: Some("pkg.safetensors".to_string()),
        }];

        app.resume_incomplete_downloads().await;

        let error = app.error.read().clone();
        assert!(
            error.as_deref().is_some_and(|e| e.contains("is missing")),
            "missing recorded root must hard-error, got: {error:?}"
        );
        assert_eq!(
            *app.status.read(),
            "Resume aborted — restore the recorded directory or delete the registry entry"
        );
        assert!(drain_downloads(&app).await.is_empty());
        assert_queue_accounting(&app, 0, 0, &[]).await;

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// plans/flare-models.md §3.3d: a FLARE-shaped entry whose recorded
    /// local_path does not end
    /// with the recorded filename is a mismatch — hard error, nothing
    /// enqueues (the registry entry's paths disagree; the user fixes or
    /// deletes the entry).
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resume_flare_entry_with_mismatched_recorded_path_aborts() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("app-resume-flare-mismatch-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        // The root exists — the SUFFIX is what mismatches here.
        let recorded_root = tmp.join("recorded").join("author").join("model");
        std::fs::create_dir_all(&recorded_root).unwrap();

        let mut app = App::new();
        app.options.hf_token = None;
        app.options.default_directory = tmp.join("live").to_string_lossy().to_string();
        app.incomplete_downloads = vec![crate::models::DownloadMetadata {
            model_id: "author/model".to_string(),
            filename: "gb202/pkg.safetensors".to_string(),
            url: "https://example.invalid/a".to_string(),
            local_path: recorded_root
                .join("other.safetensors")
                .to_string_lossy()
                .to_string(),
            total_size: 111,
            downloaded_size: 5,
            status: DownloadStatus::Incomplete,
            expected_sha256: None,
            revision: Some(FLARE_COMMIT.to_string()),
            remote_path: Some("pkg.safetensors".to_string()),
        }];

        app.resume_incomplete_downloads().await;

        let error = app.error.read().clone();
        assert!(
            error
                .as_deref()
                .is_some_and(|e| e.contains("does not end with the recorded filename")),
            "mismatched recorded path must hard-error, got: {error:?}"
        );
        assert!(drain_downloads(&app).await.is_empty());
        assert_queue_accounting(&app, 0, 0, &[]).await;

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The plans/flare-models.md §3.5 shape-test unit pins (offline, no
    /// App): the persisted-`remote_path` FLARE marker and the
    /// recorded-root derivation, including the boundary cases the flow
    /// tests above cannot reach cheaply.
    #[test]
    fn flare_shape_test_and_recorded_root_boundaries() {
        let entry = |filename: &str, revision: Option<&str>, remote_path: Option<&str>| {
            crate::models::DownloadMetadata {
                model_id: "author/model".to_string(),
                filename: filename.to_string(),
                url: "https://example.invalid/a".to_string(),
                local_path: String::new(),
                total_size: 0,
                downloaded_size: 0,
                status: DownloadStatus::Incomplete,
                expected_sha256: None,
                revision: revision.map(str::to_string),
                remote_path: remote_path.map(str::to_string),
            }
        };

        // FLARE shape is the persisted remote_path — set by every FLARE
        // frontend (CLI + TUI) at enqueue, by nothing else. Even a FLAT
        // filename with the field is flare-shaped (the field, not the
        // filename shape, is the marker).
        assert!(is_flare_shaped_entry(&entry(
            "gb202/pkg.safetensors",
            Some(FLARE_COMMIT),
            Some("pkg.safetensors")
        )));
        assert!(is_flare_shaped_entry(&entry(
            "pkg.safetensors",
            Some(FLARE_COMMIT),
            Some("pkg.safetensors")
        )));
        // Legacy entries (no remote_path — flat or nested, any revision,
        // even the old heuristic's prefixed-filename + 40-hex-commit pair)
        // stay legacy.
        assert!(!is_flare_shaped_entry(&entry("f.gguf", None, None)));
        assert!(!is_flare_shaped_entry(&entry("Q4_1/f.gguf", None, None)));
        assert!(!is_flare_shaped_entry(&entry(
            "gb202/pkg.safetensors",
            None,
            None
        )));
        assert!(!is_flare_shaped_entry(&entry(
            "gb202/pkg.safetensors",
            Some("rev-x"),
            None
        )));
        assert!(!is_flare_shaped_entry(&entry(
            "gb202/pkg.safetensors",
            Some(FLARE_COMMIT),
            None
        )));
        assert!(!is_flare_shaped_entry(&entry(
            "pkg.safetensors",
            Some(FLARE_COMMIT),
            None
        )));

        // Recorded-root derivation over a live temp dir.
        let tmp = std::env::temp_dir().join(format!("app-flare-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let root = tmp.join("base").join("author").join("model");
        std::fs::create_dir_all(root.join("gb202")).unwrap();

        let mut metadata = entry(
            "gb202/pkg.safetensors",
            Some(FLARE_COMMIT),
            Some("pkg.safetensors"),
        );
        metadata.local_path = root
            .join("gb202")
            .join("pkg.safetensors")
            .to_string_lossy()
            .to_string();
        assert_eq!(recorded_flare_root(&metadata), Ok(root.clone()));

        // A local_path with NO component above the filename suffix (a
        // bare relative path) cannot name a root — mismatch.
        metadata.local_path = format!("gb202{}pkg.safetensors", std::path::MAIN_SEPARATOR);
        assert!(recorded_flare_root(&metadata).is_err());
        // A traversal-shaped filename can never match a sanitized path.
        metadata.filename = "../pkg.safetensors".to_string();
        metadata.local_path = root.join("pkg.safetensors").to_string_lossy().to_string();
        assert!(recorded_flare_root(&metadata).is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
