//! Search + model-detail loading: query the hub, load quantizations and
//! file trees into the caches (`ApiCache::get_or_fetch`, W4.11), and drive
//! the background prefetch of adjacent models. Was `models.rs` before W3.6.
use super::state::App;
use crate::api::{build_file_tree, classify_quantizations, fetch_model_metadata};
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
        let sort_field = self.filters.sort_field;
        let sort_direction = self.filters.sort_direction;
        let min_downloads = self.filters.min_downloads;
        let min_likes = self.filters.min_likes;

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
                results
                    .iter()
                    .position(|m| m.id.to_lowercase() == query.to_lowercase())
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
            *self.status.write() = format!("Found {} models{}", models_lock.len(), filter_status);

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
            100,
            token,
        )
        .await;

        match results {
            Ok(results) => {
                // Check if query looks like a repository ID (contains /)
                let exact_match_idx = if query.contains('/') {
                    results
                        .iter()
                        .position(|m| m.id.to_lowercase() == query.to_lowercase())
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
                    crate::fmt::size_full(group.total_size),
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
                            crate::fmt::size_full(file.size),
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
        let status = self.status.clone();
        let token = self.options.hf_token.clone();

        // Spawn background task (non-blocking)
        tokio::spawn(async move {
            // Metadata cache-first via the shared get-or-fetch helper
            // (W4.11): read-lock fast path, unlocked fetch, Entry insert.
            // A failed fetch surfaces the historical error path and is
            // never cached — the next selection retries it.
            let metadata =
                match crate::models::ApiCache::get_or_fetch(&api_cache, &model_id, || {
                    fetch_model_metadata(&model_id, crate::api::DEFAULT_REVISION, token.as_ref())
                })
                .await
                {
                    Ok(meta) => meta,
                    Err(e) => {
                        *loading_quants.write() = false;
                        *error.write() = Some(format!("Failed to fetch model metadata: {}", e));

                        // Clear both states on error
                        let mut quants_lock = quantizations.write();
                        quants_lock.clear();
                        *model_metadata.write() = None;
                        *file_tree.write() = None;
                        return;
                    }
                };

            // Classify the full recursive tree (pure function — issue #25:
            // GGUFs in arbitrarily named subdirectories used to be invisible
            // because quant listing walked only the repo root).
            let groups = classify_quantizations(&metadata.siblings);

            if groups.is_empty() {
                // Standard mode: metadata + file tree (non-GGUF repos, or
                // repos with no GGUF-family files at all). This is also the
                // fallback that replaces the old dead-end empty quant panel.
                *display_mode.write() = ModelDisplayMode::Standard;

                if !metadata.siblings.is_empty() {
                    *status.write() =
                        "No quantization groups detected — showing full file tree".to_string();
                }

                // Clear quantizations (guard scoped to this statement —
                // the tree fetch below awaits, and a parking_lot write
                // guard is not Send across an await point)
                quantizations.write().clear();

                // File tree: build on cache miss, get-or-insert via the
                // shared helper (W4.11); the build is pure/infallible.
                let tree_to_store =
                    crate::models::ApiCache::get_or_fetch(&api_cache, &model_id, || async {
                        Ok::<_, std::convert::Infallible>(build_file_tree(
                            metadata.siblings.clone(),
                        ))
                    })
                    .await
                    .expect("build_file_tree is infallible");

                // Store metadata and tree in UI state
                *model_metadata.write() = Some(metadata.clone());
                *file_tree.write() = Some(tree_to_store);

                *loading_quants.write() = false;
            } else {
                // GGUF mode: show quantization groups
                *display_mode.write() = ModelDisplayMode::Gguf;

                // Quantization groups: cache-first via the shared helper
                // (W4.11) — the hit and miss paths converge on the same
                // tail (the old early-return hit arm did exactly this).
                let groups_to_store =
                    crate::models::ApiCache::get_or_fetch(&api_cache, &model_id, || async {
                        Ok::<_, std::convert::Infallible>(groups.clone())
                    })
                    .await
                    .expect("classify_quantizations output is infallible");

                let mut quants_lock = quantizations.write();
                *quants_lock = groups_to_store;
                *loading_quants.write() = false;

                // Reset file tree state
                *model_metadata.write() = None;
                *file_tree.write() = None;
            }
        });
    }

    /// Clear model details immediately (for instant UI feedback during navigation)
    pub fn clear_model_details(&mut self) {
        // Clear quantizations (GGUF mode)
        self.quantizations.write().clear();

        // Clear metadata and file tree (Standard mode)
        *self.model_metadata.write() = None;
        *self.file_tree.write() = None;

        // Set loading state
        *self.loading_quants.write() = true;
        *self.status.write() = "Loading model details...".to_string();
    }

    /// Clear search results immediately (for instant UI feedback during search)
    pub fn clear_search_results(&mut self) {
        // Clear models list
        self.models.write().clear();

        // Clear model details
        self.clear_model_details();

        // Set loading state
        *self.loading.write() = true;
        *self.status.write() = "Searching...".to_string();
    }

    /// Pre-emptively load adjacent models into cache (1 before, 1 after current selection)
    /// Loads metadata, quantizations (GGUF), and file trees (Standard) with debouncing
    pub async fn prefetch_adjacent_models(&self) {
        const PREFETCH_DEBOUNCE_MS: u128 = 1000; // Wait 1000ms before prefetching

        // Check debounce (async since W4.11 — this was the UI's last
        // futures::executor::block_on site; the only caller, App::run's
        // main loop, is already async)
        let now = std::time::Instant::now();
        let should_prefetch = {
            let mut last_time = self.last_prefetch_time.lock().await;
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
                // Metadata: cache-first via the shared helper (W4.11); a
                // failed fetch skips this model (never cached — retried
                // on a later prefetch).
                let metadata =
                    match crate::models::ApiCache::get_or_fetch(&api_cache, &model_id, || {
                        fetch_model_metadata(
                            &model_id,
                            crate::api::DEFAULT_REVISION,
                            token.as_ref(),
                        )
                    })
                    .await
                    {
                        Ok(meta) => meta,
                        Err(_) => continue, // Skip on error
                    };

                // Process based on classification of the recursive tree
                // (pure — no second fetch; issue #25)
                let groups = classify_quantizations(&metadata.siblings);

                if groups.is_empty() {
                    // Standard model: prefetch file tree
                    crate::models::ApiCache::get_or_fetch(&api_cache, &model_id, || async {
                        Ok::<_, std::convert::Infallible>(build_file_tree(
                            metadata.siblings.clone(),
                        ))
                    })
                    .await
                    .expect("build_file_tree is infallible");
                } else {
                    // GGUF model: prefetch quantization groups
                    crate::models::ApiCache::get_or_fetch(&api_cache, &model_id, || async {
                        Ok::<_, std::convert::Infallible>(groups)
                    })
                    .await
                    .expect("classify_quantizations output is infallible");
                }
            }
        });
    }
}
