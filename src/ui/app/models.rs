use super::state::App;
use crate::api::{build_file_tree, fetch_model_files, fetch_model_metadata, has_gguf_files};
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

        // Create search key for caching
        let search_key = crate::models::SearchKey {
            query: query.clone(),
            sort_field,
            sort_direction,
            min_downloads,
            min_likes,
        };

        // Check cache first
        let cached_results = {
            let cache = self.api_cache.read().unwrap();
            cache.searches.get(&search_key).cloned()
        };

        if let Some(results) = cached_results {
            // Use cached results (instant!)
            let has_results = !results.is_empty();
            let mut models_lock = models.write().unwrap();
            *models_lock = results;
            *self.loading.write().unwrap() = false;
            self.list_state.select(Some(0));

            let filter_status = if min_downloads > 0 || min_likes > 0 {
                " (cached, filtered from 100)".to_string()
            } else {
                " (cached)".to_string()
            };
            *self.status.write().unwrap() =
                format!("Found {} models{}", models_lock.len(), filter_status);

            drop(models_lock);

            if has_results {
                self.needs_load_quantizations = true;
            }
            return;
        }

        // Fetch and cache search results
        match crate::api::fetch_models_filtered(
            &query,
            sort_field,
            sort_direction,
            min_downloads,
            min_likes,
            token,
        )
        .await
        {
            Ok(results) => {
                let has_results = !results.is_empty();

                // Store in UI state
                let mut models_lock = models.write().unwrap();
                *models_lock = results.clone();
                *self.loading.write().unwrap() = false;
                self.list_state.select(Some(0));

                // Store in cache
                let mut cache = self.api_cache.write().unwrap();
                cache.searches.insert(search_key, results);
                drop(cache);

                // Show filter count in status if filters are active
                let filter_status = if min_downloads > 0 || min_likes > 0 {
                    " (filtered from 100)".to_string()
                } else {
                    String::new()
                };
                *self.status.write().unwrap() =
                    format!("Found {} models{}", models_lock.len(), filter_status);

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
        let Some(selected) = self.list_state.selected() else {
            return;
        };
        if selected >= models.len() {
            return;
        }
        let model_id = models[selected].id.clone();
        drop(models);

        // Immediate UI feedback (synchronous)
        *self.loading_quants.write().unwrap() = true;

        // Clone Arcs for background task
        let quantizations = self.quantizations.clone();
        let api_cache = self.api_cache.clone();
        let model_metadata = self.model_metadata.clone();
        let file_tree = self.file_tree.clone();
        let loading_quants = self.loading_quants.clone();
        let error = self.error.clone();
        let display_mode = self.display_mode.clone();
        let token = self.options.hf_token.clone();

        // Spawn background task (non-blocking)
        tokio::spawn(async move {
            // Check metadata cache first (avoids expensive API call)
            let cached_metadata = {
                let cache = api_cache.read().unwrap();
                cache.metadata.get(&model_id).cloned()
            };

            let metadata = if let Some(meta) = cached_metadata {
                meta // Use cached metadata
            } else {
                // Fetch and cache metadata
                match fetch_model_metadata(&model_id, token.as_ref()).await {
                    Ok(meta) => {
                        let mut cache = api_cache.write().unwrap();
                        cache.metadata.insert(model_id.clone(), meta.clone());
                        meta
                    }
                    Err(e) => {
                        *loading_quants.write().unwrap() = false;
                        *error.write().unwrap() =
                            Some(format!("Failed to fetch model metadata: {}", e));

                        // Clear both states on error
                        let mut quants_lock = quantizations.write().unwrap();
                        quants_lock.clear();
                        *model_metadata.write().unwrap() = None;
                        *file_tree.write().unwrap() = None;
                        return;
                    }
                }
            };

            // Now process based on metadata
            if true {
                // Placeholder to keep structure
                if has_gguf_files(&metadata) {
                    // GGUF mode: show quantizations
                    *display_mode.write().unwrap() = ModelDisplayMode::Gguf;

                    // Check quantization cache
                    let cached_result = {
                        let cache = api_cache.read().unwrap();
                        cache.quantizations.get(&model_id).cloned()
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
                            let mut cache = api_cache.write().unwrap();
                            cache.quantizations.insert(model_id, quants);

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

                    // Check file tree cache (avoid rebuilding)
                    let cached_tree = {
                        let cache = api_cache.read().unwrap();
                        cache.file_trees.get(&model_id).cloned()
                    };

                    let tree = if let Some(tree) = cached_tree {
                        tree // Use cached tree
                    } else {
                        // Build and cache tree
                        let tree = build_file_tree(metadata.siblings.clone());
                        let mut cache = api_cache.write().unwrap();
                        cache.file_trees.insert(model_id.clone(), tree.clone());
                        tree
                    };

                    // Store metadata and tree in UI state
                    *model_metadata.write().unwrap() = Some(metadata);
                    *file_tree.write().unwrap() = Some(tree);

                    *loading_quants.write().unwrap() = false;
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

    /// Pre-emptively load adjacent models into cache (1 before, 1 after current selection)
    /// Loads metadata, quantizations (GGUF), and file trees (Standard) with debouncing
    pub fn prefetch_adjacent_models(&self) {
        const PREFETCH_DEBOUNCE_MS: u128 = 1000; // Wait 1000ms before prefetching

        // Check debounce
        let now = std::time::Instant::now();
        let should_prefetch = {
            let mut last_time =
                futures::executor::block_on(async { self.last_prefetch_time.lock().await });
            if now.duration_since(*last_time).as_millis() > PREFETCH_DEBOUNCE_MS {
                *last_time = now;
                true
            } else {
                false
            }
        };

        if !should_prefetch {
            return; // Skip prefetch if navigating rapidly
        }

        let models = self.models.read().unwrap();
        let Some(selected) = self.list_state.selected() else {
            return;
        };
        if models.is_empty() {
            return;
        }

        // Calculate adjacent indices (1 before, 1 after)
        let mut indices_to_prefetch = Vec::new();

        if selected >= 1 {
            indices_to_prefetch.push(selected - 1);
        }
        if selected + 1 < models.len() {
            indices_to_prefetch.push(selected + 1);
        }

        // Collect model IDs
        let model_ids: Vec<String> = indices_to_prefetch
            .into_iter()
            .filter_map(|idx| models.get(idx).map(|m| m.id.clone()))
            .collect();

        drop(models);

        if model_ids.is_empty() {
            return;
        }

        // Clone Arcs for background task
        let api_cache = self.api_cache.clone();
        let token = self.options.hf_token.clone();

        // Spawn background prefetch task (fire-and-forget)
        tokio::spawn(async move {
            for model_id in model_ids {
                // Check if metadata already cached
                let metadata_cached = {
                    let cache = api_cache.read().unwrap();
                    cache.metadata.get(&model_id).cloned()
                };

                let metadata = if let Some(meta) = metadata_cached {
                    meta // Use cached
                } else {
                    // Fetch and cache metadata
                    match fetch_model_metadata(&model_id, token.as_ref()).await {
                        Ok(meta) => {
                            let mut cache = api_cache.write().unwrap();
                            cache.metadata.insert(model_id.clone(), meta.clone());
                            meta
                        }
                        Err(_) => continue, // Skip on error
                    }
                };

                // Process based on model type
                if has_gguf_files(&metadata) {
                    // GGUF model: prefetch quantizations
                    let quants_cached = {
                        let cache = api_cache.read().unwrap();
                        cache.quantizations.contains_key(&model_id)
                    };

                    if !quants_cached {
                        // Fetch and cache quantizations
                        if let Ok(quants) = fetch_model_files(&model_id, token.as_ref()).await {
                            let mut cache = api_cache.write().unwrap();
                            cache.quantizations.insert(model_id.clone(), quants);
                        }
                    }
                } else {
                    // Standard model: prefetch file tree
                    let tree_cached = {
                        let cache = api_cache.read().unwrap();
                        cache.file_trees.contains_key(&model_id)
                    };

                    if !tree_cached {
                        let tree = build_file_tree(metadata.siblings.clone());
                        let mut cache = api_cache.write().unwrap();
                        cache.file_trees.insert(model_id, tree);
                    }
                }
            }
        });
    }
}
