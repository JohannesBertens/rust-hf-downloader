use super::state::App;
use crate::api::{fetch_multipart_sha256s, parse_multipart_filename};
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
            self.status = format!("Found {} incomplete download(s)", self.incomplete_downloads.len());
        }
    }

    /// Initiate download flow - show download path popup
    pub fn trigger_download(&mut self) {
        let quantizations = futures::executor::block_on(async {
            self.quantizations.lock().await.clone()
        });
        
        if let Some(selected) = self.quant_list_state.selected() {
            if selected < quantizations.len() {
                // Update download path input with current default directory
                self.download_path_input = Input::default()
                    .with_value(self.options.default_directory.clone());
                self.popup_mode = PopupMode::DownloadPath;
                self.status = "Enter download path and press Enter".to_string();
            }
        }
    }

    /// Complete download with validation - create metadata and queue download
    pub async fn confirm_download(&mut self) {
        let models = self.models.lock().await.clone();
        let quantizations = self.quantizations.lock().await.clone();
        
        let model_selected = self.list_state.selected();
        let quant_selected = self.quant_list_state.selected();
        
        if let (Some(model_idx), Some(quant_idx)) = (model_selected, quant_selected) {
            if model_idx < models.len() && quant_idx < quantizations.len() {
                let model = &models[model_idx];
                let quant = &quantizations[quant_idx];
                
                let base_path = self.download_path_input.value().to_string();
                
                // Validate and sanitize the path to prevent path traversal
                let model_path = match validate_and_sanitize_path(&base_path, &model.id, &quant.filename) {
                    Ok(path) => path.parent().unwrap_or(&path).to_path_buf(),
                    Err(e) => {
                        self.error = Some(format!("Invalid path: {}", e));
                        self.status = "Download cancelled due to invalid path".to_string();
                        return;
                    }
                };
                
                // Check if this is a multi-part file (e.g., "00001-of-00005.gguf")
                let files_to_download = if let Some((current_part, total_parts)) = parse_multipart_filename(&quant.filename) {
                    // Generate all part filenames
                    let mut files = Vec::new();
                    for part in 1..=total_parts {
                        let part_filename = quant.filename.replace(
                            &format!("{:05}-of-{:05}", current_part, total_parts),
                            &format!("{:05}-of-{:05}", part, total_parts)
                        );
                        files.push(part_filename);
                    }
                    files
                } else {
                    vec![quant.filename.clone()]
                };
                
                let num_files = files_to_download.len();
                
                // Fetch SHA256 hashes for all parts if this is a multi-part file
                let sha256_map = if num_files > 1 {
                    match fetch_multipart_sha256s(&model.id, &files_to_download).await {
                        Ok(map) => map,
                        Err(e) => {
                            self.status = format!("Warning: Failed to fetch SHA256 hashes: {}. Downloads will proceed without verification.", e);
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
                
                for filename in &files_to_download {
                    // Validate each filename before processing
                    let validated_path = match validate_and_sanitize_path(&base_path, &model.id, filename) {
                        Ok(path) => path,
                        Err(e) => {
                            self.error = Some(format!("Invalid filename '{}': {}", filename, e));
                            continue;
                        }
                    };
                    
                    let url = format!("https://huggingface.co/{}/resolve/main/{}", model.id, filename);
                    let local_path_str = validated_path.to_string_lossy().to_string();
                    
                    // Only add if not already in registry
                    if !registry.downloads.iter().any(|d| d.url == url) {
                        // For single files, use the SHA256 from quantization info
                        // For multi-part files, look up the hash for this specific part
                        let expected_sha256 = if num_files == 1 {
                            quant.sha256.clone()
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
                for filename in &files_to_download {
                    // For single files, use the SHA256 from quantization info
                    // For multi-part files, look up the hash for this specific part
                    let sha256 = if num_files == 1 {
                        quant.sha256.clone()
                    } else {
                        // Look up hash for this specific part from already-fetched map
                        sha256_map.get(filename).and_then(|h| h.clone())
                    };
                    
                    if self.download_tx.send((
                        model.id.clone(),
                        filename.clone(),
                        model_path.clone(),
                        sha256,
                    )).is_ok() {
                        success_count += 1;
                    }
                }
                
                if success_count > 0 {
                    if num_files > 1 {
                        self.status = format!("Queued {} parts of {} to {}", num_files, quant.filename, model_path.display());
                    } else {
                        self.status = format!("Starting download of {} to {}", quant.filename, model_path.display());
                    }
                } else {
                    self.error = Some("Failed to start download".to_string());
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
        
        for metadata in &self.incomplete_downloads {
            // Queue the download to resume
            let base_path = PathBuf::from(&metadata.local_path).parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(&metadata.local_path));
            
            let _ = self.download_tx.send((
                metadata.model_id.clone(),
                metadata.filename.clone(),
                base_path,
                metadata.expected_sha256.clone(),
            ));
        }
        
        // Update queue size
        {
            let mut queue_size = self.download_queue_size.lock().await;
            *queue_size += count;
        }
        
        self.status = format!("Resuming {} incomplete download(s)", count);
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
            self.status = format!("Deleted {} incomplete file(s)", deleted);
        } else {
            self.status = format!("Deleted {} file(s), {} error(s): {}", deleted, errors.len(), errors.join(", "));
        }
        self.incomplete_downloads.clear();
    }
}
