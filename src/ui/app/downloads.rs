//! Download initiation: the confirm flows (quant group, repository, tree)
//! and resume — thin gather-ers over `EngineState::enqueue` with per-flow
//! policies (W2.1/W4.4). Observable behavior pinned byte-for-byte by the
//! characterization tests at the bottom of this file.
use super::state::App;
use crate::api::fetch_multipart_sha256s;
use crate::engine::{EnqueueOutcome, EnqueuePolicy, QueuedDownload};
use crate::models::*;
use crate::paths::sanitize::validate_and_sanitize_path;
use crate::registry;
use crate::ui::tree::count_tree_files;
use std::collections::HashMap;
use std::path::PathBuf;
use tui_input::Input;

impl App {
    /// Shared tail of the three confirm flows: surface every filename the
    /// enqueue transaction rejected (skipped from the registry but still
    /// queued and sent), then the per-flow success status or the per-flow
    /// failure error — identical control flow at every call site, only the
    /// strings differ.
    fn finish_enqueue(&mut self, outcome: &EnqueueOutcome, success: String, failure: &str) {
        for (filename, err) in &outcome.invalid {
            *self.error.write() = Some(format!("Invalid filename '{}': {}", filename, err));
        }
        if outcome.sent > 0 {
            *self.status.write() = success;
        } else {
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

                // Convert files_to_download to filenames
                let filenames_to_download: Vec<String> = files_to_download
                    .iter()
                    .map(|f| f.filename.clone())
                    .collect();

                let num_files = filenames_to_download.len();

                // Fetch multipart SHA256 hashes. The map itself is no
                // longer read (every queued part carries its own sha from
                // the quantization info; the map lookups were dead code),
                // but the fetch and its failure warning are observable,
                // so both stay.
                let _sha256_map = if num_files > 1 {
                    match fetch_multipart_sha256s(
                        &self.api_client,
                        &model.id,
                        crate::api::DEFAULT_REVISION,
                        &filenames_to_download,
                    )
                    .await
                    {
                        Ok(map) => map,
                        Err(e) => {
                            *self.status.write() = format!("Warning: Failed to fetch SHA256 hashes: {}. Downloads will proceed without verification.", e);
                            HashMap::new()
                        }
                    }
                } else {
                    HashMap::new() // Single file uses quant.sha256 directly
                };

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
                    })
                    .collect();

                // Shared enqueue transaction, GGUF-quant flavor: registry
                // mirror upsert with zero-size entries, queue accounted
                // before the sends, HUD mirror per successful send,
                // failed-send tail rollback. Files whose path fails
                // validation are skipped from the registry — and still
                // queued, exactly as before.
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

        // Files land under base/author/model (without the filename's own
        // subdirectory, e.g. "Q4_1/model.gguf" — download.rs appends it); a
        // malformed model_id falls back to the recorded local_path's
        // parent directory.
        let queued: Vec<QueuedDownload> = self
            .incomplete_downloads
            .iter()
            .map(|metadata| {
                let fallback = PathBuf::from(&metadata.local_path)
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from(&default_dir));
                let base_path = model_root_or(&default_dir, &metadata.model_id, fallback);
                QueuedDownload {
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
                }
            })
            .collect();

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

        // Load registry
        let mut registry = {
            let reg = self.engine.download_registry.lock().await;
            reg.clone()
        };

        for metadata in &self.incomplete_downloads {
            // Try to delete the actual .incomplete file
            let file_path = PathBuf::from(&metadata.local_path);
            let incomplete_path = PathBuf::from(format!("{}.incomplete", file_path.display()));

            match tokio::fs::remove_file(&incomplete_path).await {
                Ok(_) => deleted += 1,
                Err(e) => {
                    errors.push(format!("{}: {}", metadata.filename, e));
                }
            }

            // Remove from registry
            registry.downloads.retain(|d| d.url != metadata.url);
        }

        // Save updated registry
        registry::save_registry(&registry);
        {
            let mut reg = self.engine.download_registry.lock().await;
            *reg = registry;
        }

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
            })
            .collect();

        // Shared enqueue transaction, repository flavor: registry mirror
        // upsert with queued-size entries, queue accounted before the
        // sends, HUD mirror per successful send, failed-send tail rollback.
        // Files whose path fails validation are skipped from the registry —
        // and still queued, exactly as before.
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
    //! path) and HF_ENDPOINT (api_base, `fetch_multipart_sha256s`) are
    //! redirected under the crate-wide `ENV_MUTEX`; the endpoint points at
    //! a closed localhost port so the multi-part SHA fetch fails fast and
    //! deterministically (connection refused — the same observable the
    //! status overwrite hides anyway).
    use super::*;
    use crate::models::{
        DownloadStatus, LfsInfo, ModelDisplayMode, ModelInfo, ModelMetadata, PopupMode,
        QuantizationGroup, QuantizationInfo, RepoFile,
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
        let queue = app.engine.download_queue.lock().await;
        assert_eq!(queue.size, count, "queue size");
        assert_eq!(queue.bytes, bytes, "queue bytes");
        drop(queue);
        let items = app.engine.download_queue_items.lock().await;
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
        // names the FIRST file. (The transient SHA-fetch warning — the
        // endpoint is a closed port here — is overwritten by this line.)
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
}
