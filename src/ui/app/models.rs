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

        self.loading = true;
        self.error = None;
        
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
                let mut models_lock = models.lock().await;
                *models_lock = results;
                self.loading = false;
                self.list_state.select(Some(0));
                
                // Show filter count in status if filters are active
                let filter_status = if min_downloads > 0 || min_likes > 0 {
                    " (filtered from 100)".to_string()
                } else {
                    String::new()
                };
                self.status = format!("Found {} models{}", models_lock.len(), filter_status);
                
                drop(models_lock);
                
                // Trigger load for first result if we have results
                if has_results {
                    self.needs_load_quantizations = true;
                }
            }
            Err(e) => {
                self.loading = false;
                self.error = Some(format!("Failed to fetch models: {}", e));
                self.status = "Search failed".to_string();
            }
        }
    }

    /// Display detailed model information in status bar
    pub async fn show_model_details(&mut self) {
        let models = self.models.lock().await;
        if let Some(selected) = self.list_state.selected() {
            if selected < models.len() {
                let model = &models[selected];
                self.selection_info = format!(
                    "Selected: {} | URL: https://huggingface.co/{}",
                    model.id, model.id
                );
            }
        }
    }

    /// Display detailed quantization information in status bar
    pub async fn show_quantization_details(&mut self) {
        let quantizations = self.quantizations.lock().await;
        if let Some(selected) = self.quant_list_state.selected() {
            if selected < quantizations.len() {
                let group = &quantizations[selected];
                let first_file = &group.files[0];
                // Keep the model selection in line 1, show quant details in line 2
                self.status = format!(
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
                let quantizations = self.quantizations.lock().await;
                if group_idx < quantizations.len() {
                    let group = &quantizations[group_idx];
                    if file_idx < group.files.len() {
                        let file = &group.files[file_idx];
                        self.status = format!(
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
    pub async fn load_quantizations(&mut self) {
        let models = self.models.lock().await;
        if let Some(selected) = self.list_state.selected() {
            if selected < models.len() {
                let model_id = models[selected].id.clone();
                drop(models);
                
                self.loading_quants = true;
                let token = self.options.hf_token.as_ref();
                
                // Fetch model metadata first to determine display mode
                match fetch_model_metadata(&model_id, token).await {
                    Ok(metadata) => {
                        if has_gguf_files(&metadata) {
                            // GGUF mode: show quantizations
                            self.display_mode = ModelDisplayMode::Gguf;
                            
                            // Check cache first
                            let cache = self.quant_cache.lock().await;
                            if let Some(cached_groups) = cache.get(&model_id) {
                                let mut quants_lock = self.quantizations.lock().await;
                                *quants_lock = cached_groups.clone();
                                drop(cache);
                                self.loading_quants = false;
                                
                                // Reset file tree state
                                *self.model_metadata.lock().await = None;
                                *self.file_tree.lock().await = None;
                                return;
                            }
                            drop(cache);
                            
                            let quantizations = self.quantizations.clone();
                            let cache = self.quant_cache.clone();
                            
                            match fetch_model_files(&model_id, token).await {
                                Ok(quants) => {
                                    let mut quants_lock = quantizations.lock().await;
                                    *quants_lock = quants.clone();
                                    self.loading_quants = false;
                                    
                                    // Store in cache
                                    let mut cache_lock = cache.lock().await;
                                    cache_lock.insert(model_id, quants);
                                    
                                    // Reset file tree state
                                    *self.model_metadata.lock().await = None;
                                    *self.file_tree.lock().await = None;
                                }
                                Err(_) => {
                                    self.loading_quants = false;
                                    let mut quants_lock = quantizations.lock().await;
                                    quants_lock.clear();
                                }
                            }
                        } else {
                            // Standard mode: show metadata + file tree
                            self.display_mode = ModelDisplayMode::Standard;
                            
                            // Clear quantizations
                            let mut quants_lock = self.quantizations.lock().await;
                            quants_lock.clear();
                            drop(quants_lock);
                            
                            // Build file tree from siblings
                            let tree = build_file_tree(metadata.siblings.clone());
                            
                            // Store metadata and tree
                            *self.model_metadata.lock().await = Some(metadata);
                            *self.file_tree.lock().await = Some(tree);
                            
                            // Reset file tree selection
                            self.file_tree_state.select(Some(0));
                            
                            self.loading_quants = false;
                        }
                    }
                    Err(e) => {
                        self.loading_quants = false;
                        self.error = Some(format!("Failed to fetch model metadata: {}", e));
                        
                        // Clear both states on error
                        let mut quants_lock = self.quantizations.lock().await;
                        quants_lock.clear();
                        *self.model_metadata.lock().await = None;
                        *self.file_tree.lock().await = None;
                    }
                }
            }
        }
    }

    /// Clear model details immediately (for instant UI feedback during navigation)
    pub fn clear_model_details(&mut self) {
        // Clear quantizations (GGUF mode)
        futures::executor::block_on(async {
            self.quantizations.lock().await.clear();
        });
        
        // Clear metadata and file tree (Standard mode)
        futures::executor::block_on(async {
            *self.model_metadata.lock().await = None;
            *self.file_tree.lock().await = None;
        });
        
        // Set loading state
        self.loading_quants = true;
        self.status = "Loading model details...".to_string();
    }

    /// Clear search results immediately (for instant UI feedback during search)
    pub fn clear_search_results(&mut self) {
        // Clear models list
        futures::executor::block_on(async {
            self.models.lock().await.clear();
        });
        
        // Clear model details
        self.clear_model_details();
        
        // Set loading state
        self.loading = true;
        self.status = "Searching...".to_string();
    }

}
