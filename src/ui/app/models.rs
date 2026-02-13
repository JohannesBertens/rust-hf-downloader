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

        *self.loading.write() = true;
        *self.error.write() = None;

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

        // Step 1: Check cache with read lock (fast path)
        let cached_results = {
            let cache = self.api_cache.read();
            cache.searches.get(&search_key).cloned()
        };

        if let Some(results) = cached_results {
            // Use cached results (no write lock needed!)
            let exact_match_idx = if query.contains('/') {
                results.iter().position(|m| m.id.to_lowercase() == query.to_lowercase())
            } else {
                None
            };

            let has_exact_match = exact_match_idx.is_some();
            let filtered_results = if let Some(idx) = exact_match_idx {
                vec![results[idx].clone()]
            } else {
                results
            };

            let has_results = !filtered_results.is_empty();
            let mut models_lock = models.write();
            *models_lock = filtered_results;
            *self.loading.write() = false;
            self.list_state.select(Some(0));

            let filter_status = if min_downloads > 0 || min_likes > 0 {
                " (cached, filtered from 100)".to_string()
            } else if has_exact_match {
                " (cached, exact match)".to_string()
            } else {
                " (cached)".to_string()
            };
            *self.status.write() =
                format!("Found {} models{}", models_lock.len(), filter_status);

            drop(models_lock);

            if has_results {
                self.needs_load_quantizations = true;
            }
            return;
        }

        // Step 2: Fetch from API (if not cached)
        let results = crate::api::fetch_models_filtered(
            &query,
            sort_field,
            sort_direction,
            min_downloads,
            min_likes,
            token,
        )
        .await;

        match results {
            Ok(results) => {
                // Check if query looks like a repository ID (contains /)
                let exact_match_idx = if query.contains('/') {
                    results.iter().position(|m| m.id.to_lowercase() == query.to_lowercase())
                } else {
                    None
                };

                let has_exact_match = exact_match_idx.is_some();
                let filtered_results = if let Some(idx) = exact_match_idx {
                    vec![results[idx].clone()]
                } else {
                    results
                };

                let has_results = !filtered_results.is_empty();

                // Step 3: Cache results using Entry API (atomic get-or-insert with write lock)
                let results_to_store = {
                    let mut cache = self.api_cache.write();
                    match cache.searches.entry(search_key.clone()) {
                        std::collections::hash_map::Entry::Occupied(o) => o.get().clone(),
                        std::collections::hash_map::Entry::Vacant(v) => {
                            v.insert(filtered_results.clone());
                            filtered_results
                        }
                    }
                };

                // Step 4: Use the results (either our cached or another task's)
                let mut models_lock = models.write();
                *models_lock = results_to_store.clone();
                *self.loading.write() = false;
                self.list_state.select(Some(0));

                let filter_status = if min_downloads > 0 || min_likes > 0 {
                    " (filtered from 100)".to_string()
                } else if has_exact_match {
                    " (exact match)".to_string()
                } else {
                    String::new()
                };
                *self.status.write() =
                    format!("Found {} models{}", models_lock.len(), filter_status);

                drop(models_lock);

                if has_results {
                    self.needs_load_quantizations = true;
                }
            }
            Err(e) => {
                *self.loading.write() = false;
                *self.error.write() = Some(format!("Failed to fetch models: {}", e));
                *self.status.write() = "Search failed".to_string();
            }
        }
    }

    /// Display detailed model information in status bar
    pub async fn show_model_details(&mut self) {
        let models = self.models.read();
        if let Some(selected) = self.list_state.selected() {
            if selected < models.len() {
                let model = &models[selected];
                *self.selection_info.write() = format!(
                    "Selected: {} | URL: https://huggingface.co/{}",
                    model.id, model.id
                );
            }
        }
    }

    /// Display detailed quantization information in status bar
    pub async fn show_quantization_details(&mut self) {
        let quantizations = self.quantizations.read();
        if let Some(selected) = self.quant_list_state.selected() {
            if selected < quantizations.len() {
                let group = &quantizations[selected];
                let first_file = &group.files[0];
                // Keep the model selection in line 1, show quant details in line 2
                *self.status.write() = format!(
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
                let quantizations = self.quantizations.read();
                if group_idx < quantizations.len() {
                    let group = &quantizations[group_idx];
                    if file_idx < group.files.len() {
                        let file = &group.files[file_idx];
                        *self.status.write() = format!(
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
        let models = self.models.read();
        let Some(selected) = self.list_state.selected() else {
            return;
        };
        if selected >= models.len() {
            return;
        }
        let model_id = models[selected].id.clone();
        drop(models);

        // Immediate UI feedback (synchronous)
        *self.loading_quants.write() = true;

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
                let cache = api_cache.read();
                cache.metadata.get(&model_id).cloned()
            };

            let metadata = if let Some(meta) = cached_metadata {
                meta // Use cached metadata
            } else {
                // Fetch and cache metadata
                match fetch_model_metadata(&model_id, token.as_ref()).await {
                    Ok(meta) => {
                        let mut cache = api_cache.write();
                        cache.metadata.insert(model_id.clone(), meta.clone());
                        meta
                    }
                    Err(e) => {
                        *loading_quants.write() = false;
                        *error.write() =
                            Some(format!("Failed to fetch model metadata: {}", e));

                        // Clear both states on error
                        let mut quants_lock = quantizations.write();
                        quants_lock.clear();
                        *model_metadata.write() = None;
                        *file_tree.write() = None;
                        return;
                    }
                }
            };

            // Now process based on metadata
            if true {
                // Placeholder to keep structure
                if has_gguf_files(&metadata) {
                    // GGUF mode: show quantizations
                    *display_mode.write() = ModelDisplayMode::Gguf;

                    // Check quantization cache with read lock
                    let cached_result = {
                        let cache = api_cache.read();
                        cache.quantizations.get(&model_id).cloned()
                    };

                    if let Some(cached_groups) = cached_result {
                        let mut quants_lock = quantizations.write();
                        *quants_lock = cached_groups;
                        *loading_quants.write() = false;

                        // Reset file tree state
                        *model_metadata.write() = None;
                        *file_tree.write() = None;
                        return;
                    }

                    match fetch_model_files(&model_id, token.as_ref()).await {
                        Ok(quants) => {
                            // Double-check and cache using Entry API
                            let quants_to_store = {
                                let mut cache = api_cache.write();
                                match cache.quantizations.entry(model_id.clone()) {
                                    std::collections::hash_map::Entry::Occupied(o) => o.get().clone(),
                                    std::collections::hash_map::Entry::Vacant(v) => {
                                        v.insert(quants.clone());
                                        quants
                                    }
                                }
                            };

                            let mut quants_lock = quantizations.write();
                            *quants_lock = quants_to_store;
                            *loading_quants.write() = false;

                            // Reset file tree state
                            *model_metadata.write() = None;
                            *file_tree.write() = None;
                        }
                        Err(_) => {
                            *loading_quants.write() = false;
                            let mut quants_lock = quantizations.write();
                            quants_lock.clear();
                        }
                    }
                } else {
                    // Standard mode: show metadata + file tree
                    *display_mode.write() = ModelDisplayMode::Standard;

                    // Clear quantizations
                    let mut quants_lock = quantizations.write();
                    quants_lock.clear();
                    drop(quants_lock);

                    // Check file tree cache with read lock
                    let cached_tree = {
                        let cache = api_cache.read();
                        cache.file_trees.get(&model_id).cloned()
                    };

                    let tree_to_store = if let Some(tree) = cached_tree {
                        tree // Use cached tree
                    } else {
                        // Build tree
                        let tree = build_file_tree(metadata.siblings.clone());

                        // Double-check and cache using Entry API
                        let tree_to_store = {
                            let mut cache = api_cache.write();
                            match cache.file_trees.entry(model_id.clone()) {
                                std::collections::hash_map::Entry::Occupied(o) => o.get().clone(),
                                std::collections::hash_map::Entry::Vacant(v) => {
                                    v.insert(tree.clone());
                                    tree
                                }
                            }
                        };

                        tree_to_store
                    };

                    // Store metadata and tree in UI state
                    *model_metadata.write() = Some(metadata.clone());
                    *file_tree.write() = Some(tree_to_store);

                    *loading_quants.write() = false;
                }
            }
        });
    }

    /// Clear model details immediately (for instant UI feedback during navigation)
    pub fn clear_model_details(&mut self) {
        // Clear quantizations (GGUF mode)
        futures::executor::block_on(async {
            self.quantizations.write().clear();
        });

        // Clear metadata and file tree (Standard mode)
        futures::executor::block_on(async {
            *self.model_metadata.write() = None;
            *self.file_tree.write() = None;
        });

        // Set loading state
        *self.loading_quants.write() = true;
        *self.status.write() = "Loading model details...".to_string();
    }

    /// Clear search results immediately (for instant UI feedback during search)
    pub fn clear_search_results(&mut self) {
        // Clear models list
        futures::executor::block_on(async {
            self.models.write().clear();
        });

        // Clear model details
        self.clear_model_details();

        // Set loading state
        *self.loading.write() = true;
        *self.status.write() = "Searching...".to_string();
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

        let models = self.models.read();
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
                // Check metadata cache with read lock
                let metadata_cached = {
                    let cache = api_cache.read();
                    cache.metadata.get(&model_id).cloned()
                };

                let metadata = if let Some(meta) = metadata_cached {
                    meta // Use cached
                } else {
                    // Fetch and cache metadata with double-check using Entry API
                    let meta_to_store = match fetch_model_metadata(&model_id, token.as_ref()).await {
                        Ok(meta) => {
                            let mut cache = api_cache.write();
                            match cache.metadata.entry(model_id.clone()) {
                                std::collections::hash_map::Entry::Occupied(o) => o.get().clone(),
                                std::collections::hash_map::Entry::Vacant(v) => {
                                    v.insert(meta.clone());
                                    meta
                                }
                            }
                        }
                        Err(_) => continue, // Skip on error
                    };
                    meta_to_store
                };

                // Process based on model type
                if has_gguf_files(&metadata) {
                    // GGUF model: prefetch quantizations
                    let quants_cached = {
                        let cache = api_cache.read();
                        cache.quantizations.contains_key(&model_id)
                    };

                    if !quants_cached {
                        // Fetch and cache quantizations with double-check using Entry API
                        if let Ok(quants) = fetch_model_files(&model_id, token.as_ref()).await {
                            let mut cache = api_cache.write();
                            if matches!(cache.quantizations.entry(model_id.clone()), std::collections::hash_map::Entry::Vacant(_)) {
                                cache.quantizations.insert(model_id.clone(), quants);
                            }
                        }
                    }
                } else {
                    // Standard model: prefetch file tree
                    let tree_cached = {
                        let cache = api_cache.read();
                        cache.file_trees.contains_key(&model_id)
                    };

                    if !tree_cached {
                        // Build and cache file tree with double-check using Entry API
                        let tree = build_file_tree(metadata.siblings.clone());
                        let mut cache = api_cache.write();
                        if matches!(cache.file_trees.entry(model_id.clone()), std::collections::hash_map::Entry::Vacant(_)) {
                            cache.file_trees.insert(model_id.clone(), tree);
                        }
                    }
                }
            }
        });
    }
}
