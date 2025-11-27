use super::state::App;
use crate::api::{fetch_model_files, fetch_model_metadata, has_gguf_files, build_file_tree};
use crate::models::ModelDisplayMode;

impl App {
    /// Execute search query and load results
    pub async fn search_models(&mut self) {
        let query = self.input.value().to_string();
        
        if query.is_empty() {
            return;
        }

        *self.loading.write().unwrap() = true;
        *self.error.write().unwrap() = None;
        
        let models = self.models.clone();
        let token = self.options.hf_token.as_ref();
        let sort_field = self.sort_field;
        let sort_direction = self.sort_direction;
        let min_downloads = self.filter_min_downloads;
        let min_likes = self.filter_min_likes;
        
        // Use fetch_models_filtered with current sort and filter settings
        match crate::api::fetch_models_filtered(
            &query,
            sort_field,
            sort_direction,
            min_downloads,
            min_likes,
            token
        ).await {
            Ok(results) => {
                let has_results = !results.is_empty();
                let mut models_lock = models.write().unwrap();
                *models_lock = results;
                *self.loading.write().unwrap() = false;
                self.list_state.select(Some(0));
                
                // Show filter count in status if filters are active
                let filter_status = if min_downloads > 0 || min_likes > 0 {
                    " (filtered from 100)".to_string()
                } else {
                    String::new()
                };
                *self.status.write().unwrap() = format!("Found {} models{}", models_lock.len(), filter_status);
                
                drop(models_lock);
                
                // Trigger load for first result if we have results
                if has_results {
                    self.needs_load_quantizations = true;
                }
            }
            Err(e) => {
                *self.loading.write().unwrap() = false;
                *self.error.write().unwrap() = Some(format!("Failed to fetch models: {}", e));
                *self.status.write().unwrap() = "Search failed".to_string();
            }
        }
    }

    /// Display detailed model information in status bar
    pub async fn show_model_details(&mut self) {
        let models = self.models.read().unwrap();
        if let Some(selected) = self.list_state.selected() {
            if selected < models.len() {
                let model = &models[selected];
                *self.selection_info.write().unwrap() = format!(
                    "Selected: {} | URL: https://huggingface.co/{}",
                    model.id, model.id
                );
            }
        }
    }

    /// Display detailed quantization information in status bar
    pub async fn show_quantization_details(&mut self) {
        let quantizations = self.quantizations.read().unwrap();
        if let Some(selected) = self.quant_list_state.selected() {
            if selected < quantizations.len() {
                let group = &quantizations[selected];
                let first_file = &group.files[0];
                // Keep the model selection in line 1, show quant details in line 2
                *self.status.write().unwrap() = format!(
                    "Type: {} | Size: {} | File: {}",
                    group.quant_type,
                    crate::utils::format_size(group.total_size),
                    first_file.filename
                );
            }
        }
    }

    pub async fn show_file_details(&mut self) {
        if let Some(group_idx) = self.quant_list_state.selected() {
            if let Some(file_idx) = self.quant_file_list_state.selected() {
                let quantizations = self.quantizations.read().unwrap();
                if group_idx < quantizations.len() {
                    let group = &quantizations[group_idx];
                    if file_idx < group.files.len() {
                        let file = &group.files[file_idx];
                        *self.status.write().unwrap() = format!(
                            "File: {} | Size: {} | Type: {}",
                            file.filename,
                            crate::utils::format_size(file.size),
                            file.quant_type
                        );
                    }
                }
            }
        }
    }

    /// Load quantizations for currently selected model (with cache check)
    /// Now supports dual-mode: GGUF quantizations or standard model metadata + file tree
    /// Spawns a background task to avoid blocking UI thread
    pub fn spawn_load_quantizations(&mut self) {
        // Get selected model synchronously
        let models = self.models.read().unwrap();
        let Some(selected) = self.list_state.selected() else { return };
        if selected >= models.len() { return }
        let model_id = models[selected].id.clone();
        drop(models);
        
        // Immediate UI feedback (synchronous)
        *self.loading_quants.write().unwrap() = true;
        
        // Clone Arcs for background task
        let quantizations = self.quantizations.clone();
        let quant_cache = self.quant_cache.clone();
        let model_metadata = self.model_metadata.clone();
        let file_tree = self.file_tree.clone();
        let loading_quants = self.loading_quants.clone();
        let error = self.error.clone();
        let display_mode = self.display_mode.clone();
        let token = self.options.hf_token.clone();
        
        // Spawn background task (non-blocking)
        tokio::spawn(async move {
            // Fetch model metadata first to determine display mode
            match fetch_model_metadata(&model_id, token.as_ref()).await {
                Ok(metadata) => {
                    if has_gguf_files(&metadata) {
                        // GGUF mode: show quantizations
                        *display_mode.write().unwrap() = ModelDisplayMode::Gguf;
                        
                        // Check cache first
                        let cached_result = {
                            let cache = quant_cache.read().unwrap();
                            cache.get(&model_id).cloned()
                        };
                        
                        if let Some(cached_groups) = cached_result {
                            let mut quants_lock = quantizations.write().unwrap();
                            *quants_lock = cached_groups;
                            *loading_quants.write().unwrap() = false;
                            
                            // Reset file tree state
                            *model_metadata.write().unwrap() = None;
                            *file_tree.write().unwrap() = None;
                            return;
                        }
                        
                        match fetch_model_files(&model_id, token.as_ref()).await {
                            Ok(quants) => {
                                let mut quants_lock = quantizations.write().unwrap();
                                *quants_lock = quants.clone();
                                *loading_quants.write().unwrap() = false;
                                
                                // Store in cache
                                let mut cache_lock = quant_cache.write().unwrap();
                                cache_lock.insert(model_id, quants);
                                
                                // Reset file tree state
                                *model_metadata.write().unwrap() = None;
                                *file_tree.write().unwrap() = None;
                            }
                            Err(_) => {
                                *loading_quants.write().unwrap() = false;
                                let mut quants_lock = quantizations.write().unwrap();
                                quants_lock.clear();
                            }
                        }
                    } else {
                        // Standard mode: show metadata + file tree
                        *display_mode.write().unwrap() = ModelDisplayMode::Standard;
                        
                        // Clear quantizations
                        let mut quants_lock = quantizations.write().unwrap();
                        quants_lock.clear();
                        drop(quants_lock);
                        
                        // Build file tree from siblings
                        let tree = build_file_tree(metadata.siblings.clone());
                        
                        // Store metadata and tree
                        *model_metadata.write().unwrap() = Some(metadata);
                        *file_tree.write().unwrap() = Some(tree);
                        
                        *loading_quants.write().unwrap() = false;
                    }
                }
                Err(e) => {
                    *loading_quants.write().unwrap() = false;
                    *error.write().unwrap() = Some(format!("Failed to fetch model metadata: {}", e));
                    
                    // Clear both states on error
                    let mut quants_lock = quantizations.write().unwrap();
                    quants_lock.clear();
                    *model_metadata.write().unwrap() = None;
                    *file_tree.write().unwrap() = None;
                }
            }
        });
    }

    /// Clear model details immediately (for instant UI feedback during navigation)
    pub fn clear_model_details(&mut self) {
        // Clear quantizations (GGUF mode)
        futures::executor::block_on(async {
            self.quantizations.write().unwrap().clear();
        });
        
        // Clear metadata and file tree (Standard mode)
        futures::executor::block_on(async {
            *self.model_metadata.write().unwrap() = None;
            *self.file_tree.write().unwrap() = None;
        });
        
        // Set loading state
        *self.loading_quants.write().unwrap() = true;
        *self.status.write().unwrap() = "Loading model details...".to_string();
    }

    /// Clear search results immediately (for instant UI feedback during search)
    pub fn clear_search_results(&mut self) {
        // Clear models list
        futures::executor::block_on(async {
            self.models.write().unwrap().clear();
        });
        
        // Clear model details
        self.clear_model_details();
        
        // Set loading state
        *self.loading.write().unwrap() = true;
        *self.status.write().unwrap() = "Searching...".to_string();
    }

}
