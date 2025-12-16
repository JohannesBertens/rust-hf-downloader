use super::state::App;
use crate::api::fetch_multipart_sha256s;
use crate::download::validate_and_sanitize_path;
use crate::models::*;
use crate::registry;
use std::collections::HashMap;
use std::path::PathBuf;
use tui_input::Input;

impl App {
    /// Scan registry for incomplete downloads and show resume popup if found
    pub async fn scan_incomplete_downloads(&mut self) {
        // Load registry from disk
        let registry = registry::load_registry();
        
        // Update the app's registry
        {
            let mut reg = self.download_registry.lock().await;
            *reg = registry.clone();
        }
        
        // Find incomplete downloads
        self.incomplete_downloads = registry::get_incomplete_downloads(&registry);
        
        // Load complete downloads into memory
        let complete_map = registry::get_complete_downloads(&registry);
        
        {
            let mut complete = self.complete_downloads.lock().await;
            *complete = complete_map;
        }
        
        // Show popup if incomplete downloads found
        if !self.incomplete_downloads.is_empty() {
            self.popup_mode = PopupMode::ResumeDownload;
            *self.status.write().unwrap() = format!("Found {} incomplete download(s)", self.incomplete_downloads.len());
        }
    }

    /// Initiate download flow - show download path popup
    pub fn trigger_download(&mut self) {
        // Check which pane is focused to determine what to download
        match self.focused_pane {
            FocusedPane::Models => {
                // Download entire model repository (non-GGUF models in Standard mode)
                if *self.display_mode.read().unwrap() == crate::models::ModelDisplayMode::Standard {
                    let metadata = futures::executor::block_on(async {
                        self.model_metadata.read().unwrap().clone()
                    });
                    
                    if let Some(meta) = metadata {
                        let file_count = meta.siblings.len();
                        self.download_path_input = Input::default()
                            .with_value(self.options.default_directory.clone());
                        self.popup_mode = PopupMode::DownloadPath;
                        *self.status.write().unwrap() = format!("Download all {} files from repository", file_count);
                    }
                }
            }
            FocusedPane::QuantizationGroups => {
                // Download entire quantization group
                let quantizations = futures::executor::block_on(async {
                    self.quantizations.read().unwrap().clone()
                });
                
                if let Some(selected) = self.quant_list_state.selected() {
                    if selected < quantizations.len() {
                        // Update download path input with current default directory
                        self.download_path_input = Input::default()
                            .with_value(self.options.default_directory.clone());
                        self.popup_mode = PopupMode::DownloadPath;
                        *self.status.write().unwrap() = format!("Download all {} files in quantization group", quantizations[selected].files.len());
                    }
                }
            }
            FocusedPane::QuantizationFiles => {
                // Download specific file only
                if let Some(_group_idx) = self.quant_list_state.selected() {
                    if let Some(_file_idx) = self.quant_file_list_state.selected() {
                        self.download_path_input = Input::default()
                            .with_value(self.options.default_directory.clone());
                        self.popup_mode = PopupMode::DownloadPath;
                        *self.status.write().unwrap() = "Download single selected file".to_string();
                    }
                }
            }
            _ => {}
        }
    }

    /// Complete download with validation - create metadata and queue download
    pub async fn confirm_download(&mut self) {
        // Check if we're downloading a full repository (non-GGUF model)
        if self.focused_pane == FocusedPane::Models && 
           *self.display_mode.read().unwrap() == crate::models::ModelDisplayMode::Standard {
            self.confirm_repository_download().await;
            return;
        }
        
        let models = self.models.read().unwrap().clone();
        let quant_groups = self.quantizations.read().unwrap().clone();
        
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
                    *self.error.write().unwrap() = Some("No files selected for download".to_string());
                    return;
                }
                
                let quant = &files_to_download[0];
                
                let base_path = self.download_path_input.value().to_string();
                
                // Validate the path to prevent path traversal
                if let Err(e) = validate_and_sanitize_path(&base_path, &model.id, &quant.filename) {
                    *self.error.write().unwrap() = Some(format!("Invalid path: {}", e));
                    *self.status.write().unwrap() = "Download cancelled due to invalid path".to_string();
                    return;
                }
                
                // Calculate model_path as base/author/model_name (without file's subdirectory)
                // The filename may contain subdirectories (e.g., "UD-Q6_K_XL/model.gguf")
                // which will be appended during download, so we don't include them here
                let model_parts: Vec<&str> = model.id.split('/').collect();
                let model_path = if model_parts.len() == 2 {
                    PathBuf::from(&base_path).join(model_parts[0]).join(model_parts[1])
                } else {
                    PathBuf::from(&base_path)
                };
                
                // Convert files_to_download to filenames
                let filenames_to_download: Vec<String> = files_to_download.iter()
                    .map(|f| f.filename.clone())
                    .collect();
                
                let num_files = filenames_to_download.len();
                
                // Fetch SHA256 hashes for all files
                let token = self.options.hf_token.as_ref();
                let sha256_map = if num_files > 1 {
                    match fetch_multipart_sha256s(&model.id, &filenames_to_download, token).await {
                        Ok(map) => map,
                        Err(e) => {
                            *self.status.write().unwrap() = format!("Warning: Failed to fetch SHA256 hashes: {}. Downloads will proceed without verification.", e);
                            HashMap::new()
                        }
                    }
                } else {
                    HashMap::new() // Single file uses quant.sha256 directly
                };
                
                // Load registry and add metadata entries for all files
                let mut registry = {
                    let reg = self.download_registry.lock().await;
                    reg.clone()
                };
                
                for (idx, filename) in filenames_to_download.iter().enumerate() {
                    // Validate each filename before processing
                    let validated_path = match validate_and_sanitize_path(&base_path, &model.id, filename) {
                        Ok(path) => path,
                        Err(e) => {
                            *self.error.write().unwrap() = Some(format!("Invalid filename '{}': {}", filename, e));
                            continue;
                        }
                    };
                    
                    let url = format!("https://huggingface.co/{}/resolve/main/{}", model.id, filename);
                    let local_path_str = validated_path.to_string_lossy().to_string();
                    
                    // Only add if not already in registry
                    if !registry.downloads.iter().any(|d| d.url == url) {
                        // Get SHA256 from the corresponding QuantizationInfo
                        let expected_sha256 = if idx < files_to_download.len() {
                            files_to_download[idx].sha256.clone()
                        } else if num_files == 1 {
                            files_to_download[0].sha256.clone()
                        } else {
                            // Look up hash for this specific part from fetched map
                            sha256_map.get(filename).and_then(|h| h.clone())
                        };
                        
                        registry.downloads.push(DownloadMetadata {
                            model_id: model.id.clone(),
                            filename: filename.clone(),
                            url: url.clone(),
                            local_path: local_path_str,
                            total_size: 0,
                            downloaded_size: 0,
                            status: DownloadStatus::Incomplete,
                            expected_sha256,
                        });
                    }
                }
                
                // Save registry with all new entries
                registry::save_registry(&registry);
                {
                    let mut reg = self.download_registry.lock().await;
                    *reg = registry;
                }
                
                // Increment queue size by number of files
                {
                    let mut queue_size = self.download_queue_size.lock().await;
                    *queue_size += num_files;
                }
                
                // Send all download requests
                let mut success_count = 0;
                let hf_token = self.options.hf_token.clone();
                for (idx, filename) in filenames_to_download.iter().enumerate() {
                    // Get SHA256 from the corresponding QuantizationInfo
                    let sha256 = if idx < files_to_download.len() {
                        files_to_download[idx].sha256.clone()
                    } else {
                        // Fallback: look up hash from fetched map
                        sha256_map.get(filename).and_then(|h| h.clone())
                    };
                    
                    if self.download_tx.send((
                        model.id.clone(),
                        filename.clone(),
                        model_path.clone(),
                        sha256,
                        hf_token.clone(),
                    )).is_ok() {
                        success_count += 1;
                    }
                }
                
                if success_count > 0 {
                    if num_files > 1 {
                        *self.status.write().unwrap() = format!("Queued {} parts of {} to {}", num_files, quant.filename, model_path.display());
                    } else {
                        *self.status.write().unwrap() = format!("Starting download of {} to {}", quant.filename, model_path.display());
                    }
                } else {
                    *self.error.write().unwrap() = Some("Failed to start download".to_string());
                }
                
                // Adjust queue size if some sends failed
                if success_count < num_files {
                    let mut queue_size = self.download_queue_size.lock().await;
                    *queue_size = queue_size.saturating_sub(num_files - success_count);
                }
            }
        }
    }

    /// Resume all incomplete downloads from registry
    pub async fn resume_incomplete_downloads(&mut self) {
        let count = self.incomplete_downloads.len();
        let hf_token = self.options.hf_token.clone();
        let default_dir = self.options.default_directory.clone();
        
        for metadata in &self.incomplete_downloads {
            // Calculate model_path as base/author/model_name (without file's subdirectory)
            // The filename may contain subdirectories (e.g., "Q4_1/model.gguf")
            // which will be appended during download
            let model_parts: Vec<&str> = metadata.model_id.split('/').collect();
            let base_path = if model_parts.len() == 2 {
                PathBuf::from(&default_dir).join(model_parts[0]).join(model_parts[1])
            } else {
                // Fallback to deriving from local_path if model_id format is unexpected
                PathBuf::from(&metadata.local_path).parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from(&default_dir))
            };
            
            let _ = self.download_tx.send((
                metadata.model_id.clone(),
                metadata.filename.clone(),
                base_path,
                metadata.expected_sha256.clone(),
                hf_token.clone(),
            ));
        }
        
        // Update queue size
        {
            let mut queue_size = self.download_queue_size.lock().await;
            *queue_size += count;
        }
        
        *self.status.write().unwrap() = format!("Resuming {} incomplete download(s)", count);
        self.incomplete_downloads.clear();
    }

    /// Delete incomplete files and remove from registry
    pub async fn delete_incomplete_downloads(&mut self) {
        let mut deleted = 0;
        let mut errors = Vec::new();
        
        // Load registry
        let mut registry = {
            let reg = self.download_registry.lock().await;
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
            let mut reg = self.download_registry.lock().await;
            *reg = registry;
        }
        
        if errors.is_empty() {
            *self.status.write().unwrap() = format!("Deleted {} incomplete file(s)", deleted);
        } else {
            *self.status.write().unwrap() = format!("Deleted {} file(s), {} error(s): {}", deleted, errors.len(), errors.join(", "));
        }
        self.incomplete_downloads.clear();
    }

    /// Download entire repository (non-GGUF models)
    pub async fn confirm_repository_download(&mut self) {
        let models = self.models.read().unwrap().clone();
        let metadata = self.model_metadata.read().unwrap().clone();
        
        let model_selected = self.list_state.selected();
        
        if let (Some(model_idx), Some(meta)) = (model_selected, metadata) {
            if model_idx < models.len() {
                let model = &models[model_idx];
                let base_path = self.download_path_input.value().to_string();
                
                // Filter out directories - only download files
                let files_to_download: Vec<_> = meta.siblings.iter()
                    .filter(|f| {
                        // Skip if it's likely a directory (no size or ends with /)
                        f.size.is_some() && !f.rfilename.ends_with('/')
                    })
                    .collect();
                
                if files_to_download.is_empty() {
                    *self.error.write().unwrap() = Some("No files to download in this repository".to_string());
                    return;
                }
                
                let num_files = files_to_download.len();
                
                // Load registry
                let mut registry = {
                    let reg = self.download_registry.lock().await;
                    reg.clone()
                };
                
                // Add metadata entries for all files
                for file in &files_to_download {
                    let filename = &file.rfilename;
                    
                    // Validate path
                    let validated_path = match validate_and_sanitize_path(&base_path, &model.id, filename) {
                        Ok(path) => path,
                        Err(e) => {
                            *self.error.write().unwrap() = Some(format!("Invalid filename '{}': {}", filename, e));
                            continue;
                        }
                    };
                    
                    let url = format!("https://huggingface.co/{}/resolve/main/{}", model.id, filename);
                    let local_path_str = validated_path.to_string_lossy().to_string();
                    
                    // Only add if not already in registry
                    if !registry.downloads.iter().any(|d| d.url == url) {
                        // Extract SHA256 from LFS info if available
                        let expected_sha256 = file.lfs.as_ref().map(|lfs| lfs.oid.clone());
                        
                        registry.downloads.push(DownloadMetadata {
                            model_id: model.id.clone(),
                            filename: filename.clone(),
                            url: url.clone(),
                            local_path: local_path_str,
                            total_size: file.size.unwrap_or(0),
                            downloaded_size: 0,
                            status: DownloadStatus::Incomplete,
                            expected_sha256,
                        });
                    }
                }
                
                // Save registry with all new entries
                registry::save_registry(&registry);
                {
                    let mut reg = self.download_registry.lock().await;
                    *reg = registry;
                }
                
                // Increment queue size
                {
                    let mut queue_size = self.download_queue_size.lock().await;
                    *queue_size += num_files;
                }
                
                // Calculate the model root directory (base/author/model_name)
                // This is where all files will be organized with their subdirectory structure
                let model_parts: Vec<&str> = model.id.split('/').collect();
                let model_root = if model_parts.len() == 2 {
                    PathBuf::from(&base_path).join(model_parts[0]).join(model_parts[1])
                } else {
                    PathBuf::from(&base_path)
                };
                
                // Send all download requests - each file will preserve its subdirectory structure
                let mut success_count = 0;
                let hf_token = self.options.hf_token.clone();
                for file in &files_to_download {
                    let sha256 = file.lfs.as_ref().map(|lfs| lfs.oid.clone());
                    
                    if self.download_tx.send((
                        model.id.clone(),
                        file.rfilename.clone(),
                        model_root.clone(),
                        sha256,
                        hf_token.clone(),
                    )).is_ok() {
                        success_count += 1;
                    }
                }
                
                if success_count > 0 {
                    *self.status.write().unwrap() = format!("Queued {} files from {} to {}", success_count, model.id, model_root.display());
                } else {
                    *self.error.write().unwrap() = Some("Failed to start downloads".to_string());
                }
                
                // Adjust queue size if some sends failed
                if success_count < num_files {
                    let mut queue_size = self.download_queue_size.lock().await;
                    *queue_size = queue_size.saturating_sub(num_files - success_count);
                }
            }
        }
    }
}
