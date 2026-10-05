use super::state::App;
use crate::api::fetch_multipart_sha256s;
use crate::engine::{EnqueuePolicy, QueuedDownload};
use crate::models::*;
use crate::paths::sanitize::validate_and_sanitize_path;
use crate::registry;
use std::collections::HashMap;
use std::path::PathBuf;
use tui_input::Input;

impl App {
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
                let flat = crate::ui::render::flatten_tree_for_navigation(&tree);
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
                let model_parts: Vec<&str> = model.id.split('/').collect();
                let model_path = if model_parts.len() == 2 {
                    PathBuf::from(&base_path)
                        .join(model_parts[0])
                        .join(model_parts[1])
                } else {
                    PathBuf::from(&base_path)
                };

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
                let token = self.options.hf_token.as_ref();
                let _sha256_map = if num_files > 1 {
                    match fetch_multipart_sha256s(
                        &model.id,
                        crate::api::DEFAULT_REVISION,
                        &filenames_to_download,
                        token,
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

                for (filename, err) in &outcome.invalid {
                    *self.error.write() = Some(format!("Invalid filename '{}': {}", filename, err));
                }

                if outcome.sent > 0 {
                    if num_files > 1 {
                        *self.status.write() = format!(
                            "Queued {} parts of {} to {}",
                            num_files,
                            quant.filename,
                            model_path.display()
                        );
                    } else {
                        *self.status.write() = format!(
                            "Starting download of {} to {}",
                            quant.filename,
                            model_path.display()
                        );
                    }
                } else {
                    *self.error.write() = Some("Failed to start download".to_string());
                }
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
                let model_parts: Vec<&str> = metadata.model_id.split('/').collect();
                let base_path = if model_parts.len() == 2 {
                    PathBuf::from(&default_dir)
                        .join(model_parts[0])
                        .join(model_parts[1])
                } else {
                    PathBuf::from(&metadata.local_path)
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from(&default_dir))
                };
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
        let models = self.models.read().clone();
        let metadata = self.model_metadata.read().clone();

        let model_selected = self.list_state.selected();

        if let (Some(model_idx), Some(meta)) = (model_selected, metadata) {
            if model_idx < models.len() {
                let model = &models[model_idx];
                let base_path = self.download_path_input.value().to_string();

                // Filter out directories - only download files
                let files_to_download: Vec<_> = meta
                    .siblings
                    .iter()
                    .filter(|f| {
                        // Skip if it's likely a directory (no size or ends with /)
                        f.size.is_some() && !f.rfilename.ends_with('/')
                    })
                    .collect();

                if files_to_download.is_empty() {
                    *self.error.write() =
                        Some("No files to download in this repository".to_string());
                    return;
                }

                // Files land under base/author/model, each file
                // preserving its subdirectory structure.
                let model_parts: Vec<&str> = model.id.split('/').collect();
                let model_root = if model_parts.len() == 2 {
                    PathBuf::from(&base_path)
                        .join(model_parts[0])
                        .join(model_parts[1])
                } else {
                    PathBuf::from(&base_path)
                };

                // Queue payload; the registry entries are derived from
                // these same fields by the enqueue transaction below.
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

                // Shared enqueue transaction, repository flavor: registry
                // mirror upsert with queued-size entries, queue accounted
                // before the sends, HUD mirror per successful send,
                // failed-send tail rollback. Files whose path fails
                // validation are skipped from the registry — and still
                // queued, exactly as before.
                let outcome = self
                    .engine
                    .enqueue(
                        &self.download_tx,
                        &queued,
                        &EnqueuePolicy::tui_repository(&base_path),
                    )
                    .await;

                for (filename, err) in &outcome.invalid {
                    *self.error.write() = Some(format!("Invalid filename '{}': {}", filename, err));
                }

                if outcome.sent > 0 {
                    *self.status.write() = format!(
                        "Queued {} files from {} to {}",
                        outcome.sent,
                        model.id,
                        model_root.display()
                    );
                } else {
                    *self.error.write() = Some("Failed to start downloads".to_string());
                }
            }
        }
    }

    /// Queue a download for a Standard-mode tree selection: a single file
    /// (`path`, `is_dir == false`) or every file under a directory
    /// (`path`, `is_dir == true`). Issue #25 P5: non-GGUF repos previously
    /// only offered whole-repo download.
    async fn confirm_tree_download(&mut self, path: &str, is_dir: bool) {
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

        // Select the file itself, or every file inside the directory
        let prefix = format!("{}/", path.trim_end_matches('/'));
        let files_to_download: Vec<_> = meta
            .siblings
            .iter()
            .filter(|f| {
                f.size.is_some()
                    && !f.rfilename.ends_with('/')
                    && if is_dir {
                        f.rfilename.starts_with(&prefix)
                    } else {
                        f.rfilename == path
                    }
            })
            .collect();

        if files_to_download.is_empty() {
            *self.error.write() = Some(format!("No downloadable files match {}", path));
            *self.status.write() = "Download cancelled".to_string();
            return;
        }

        // Files land under base/author/model/<repo subpath>
        let model_parts: Vec<&str> = model.id.split('/').collect();
        let model_root = if model_parts.len() == 2 {
            PathBuf::from(&base_path)
                .join(model_parts[0])
                .join(model_parts[1])
        } else {
            PathBuf::from(&base_path)
        };

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

        // Shared enqueue transaction, repository flavor (see
        // confirm_repository_download): registry mirror upsert with
        // queued-size entries, queue accounted before the sends, HUD mirror
        // per successful send, failed-send tail rollback. Files whose path
        // fails validation are skipped from the registry — and still
        // queued, exactly as before.
        let outcome = self
            .engine
            .enqueue(
                &self.download_tx,
                &queued,
                &EnqueuePolicy::tui_repository(&base_path),
            )
            .await;

        for (filename, err) in &outcome.invalid {
            *self.error.write() = Some(format!("Invalid filename '{}': {}", filename, err));
        }

        if outcome.sent > 0 {
            *self.status.write() = format!(
                "Queued {} file{} from {} to {}",
                outcome.sent,
                if outcome.sent == 1 { "" } else { "s" },
                path,
                model_root.display()
            );
        } else {
            *self.error.write() = Some("Failed to start downloads".to_string());
        }
    }
}

/// Recursively count file nodes under a tree node (for the download popup
/// label).
fn count_tree_files(node: &FileTreeNode) -> usize {
    if node.is_dir {
        node.children.iter().map(count_tree_files).sum()
    } else {
        1
    }
}
