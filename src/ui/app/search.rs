//! Search + model-detail loading: query the hub, load quantizations and
//! file trees into the caches (`ApiCache::get_or_fetch`, W4.11), and drive
//! the background prefetch of adjacent models. Was `models.rs` before W3.6.
//!
//! plans/flare-models.md §3.4 adds the FLARE detail scan: disposition over
//! the fetched
//! metadata → (Probable confirmed via raw `config.json`) → cached
//! refs+trees scan → ownership-guarded publish into `ModelDisplayMode::Flare`.
use super::state::App;
use crate::api::{
    build_file_tree, classify_quantizations, config_json_confirms_flare, fetch_flare_info,
    fetch_model_metadata, fetch_raw_file, flare_disposition, FlareDisposition,
};
use crate::models::{ModelDisplayMode, RepoFile};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The shared state slots a FLARE detail scan writes
/// (plans/flare-models.md §3.4): bundled so
/// the spawned scan task (which owns Arc clones, not `&App`) and the
/// offline tests share ONE ownership-guarded publish body. The epoch is
/// [`App::flare_scan_epoch`] — bumped on every selection change and
/// every new scan spawn; `publish` is a no-op for a superseded epoch.
pub(super) struct FlareScanShared {
    pub epoch: Arc<AtomicU64>,
    pub display_mode: Arc<parking_lot::RwLock<ModelDisplayMode>>,
    pub flare_info: Arc<parking_lot::RwLock<Option<crate::models::FlareRepoInfo>>>,
    pub quantizations: Arc<parking_lot::RwLock<Vec<crate::models::QuantizationGroup>>>,
    pub model_metadata: Arc<parking_lot::RwLock<Option<crate::models::ModelMetadata>>>,
    pub file_tree: Arc<parking_lot::RwLock<Option<crate::models::FileTreeNode>>>,
    pub loading_quants: Arc<parking_lot::RwLock<bool>>,
    pub status: Arc<parking_lot::RwLock<String>>,
}

impl FlareScanShared {
    /// Capture the publish slots off `App` (spawn time).
    fn from_app(app: &App) -> Self {
        Self {
            epoch: app.flare_scan_epoch.clone(),
            display_mode: app.display_mode.clone(),
            flare_info: app.flare_info.clone(),
            quantizations: app.quantizations.clone(),
            model_metadata: app.model_metadata.clone(),
            file_tree: app.file_tree.clone(),
            loading_quants: app.loading_quants.clone(),
            status: app.status.clone(),
        }
    }

    /// plans/flare-models.md §3.4 ownership-guarded publish: installs the
    /// scan result ONLY while
    /// `epoch` is still current. Publishing switches to Flare mode,
    /// installs `flare_info`, clears the GGUF/Standard state (the converse
    /// of what the Standard/GGUF paths do to `flare_info`), drops the
    /// spinner, and notes the revision count. A superseded scan is a
    /// silent no-op — the current selection's state wins.
    pub(super) fn publish(&self, epoch: u64, info: crate::models::FlareRepoInfo) {
        if self.epoch.load(Ordering::Relaxed) != epoch {
            return;
        }
        let revisions = info.revisions.len();
        *self.display_mode.write() = ModelDisplayMode::Flare;
        *self.flare_info.write() = Some(info);
        self.quantizations.write().clear();
        *self.model_metadata.write() = None;
        *self.file_tree.write() = None;
        *self.loading_quants.write() = false;
        *self.status.write() =
            format!("FLARE repository: {revisions} payload revision(s) — press 'd' to download");
    }
}

/// Confirm a Probable FLARE disposition (plans/flare-models.md §3.4): fetch
/// the raw
/// `config.json` at main and apply the live-verified markers. The LFS
/// guard uses the already-fetched main tree (`siblings`) — an LFS or
/// absent `config.json` is not confirmable; a failed fetch is NOT an
/// error (degrades to the Standard/GGUF classification).
async fn confirm_flare_probable(
    client: &reqwest::Client,
    model_id: &str,
    siblings: &[RepoFile],
) -> bool {
    let plain_config = siblings
        .iter()
        .any(|file| file.rfilename == "config.json" && file.lfs.is_none());
    if !plain_config {
        return false;
    }
    match fetch_raw_file(
        client,
        model_id,
        crate::api::DEFAULT_REVISION,
        "config.json",
    )
    .await
    {
        Ok(body) => config_json_confirms_flare(Some(&body)),
        Err(_) => false,
    }
}

impl App {
    /// Capture the current FLARE scan epoch for a newly spawned detail
    /// scan (plans/flare-models.md §3.4): every spawn advances the epoch,
    /// superseding any
    /// earlier in-flight scan.
    fn begin_flare_scan(&self) -> u64 {
        self.flare_scan_epoch.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Invalidate every in-flight FLARE scan (plans/flare-models.md §3.4):
    /// run from
    /// `clear_model_details` on every selection change, so a scan for a
    /// previously selected model can never publish over the new one.
    fn invalidate_flare_scans(&self) {
        self.flare_scan_epoch.fetch_add(1, Ordering::Relaxed);
    }

    /// Execute search query and load results
    pub async fn search_models(&mut self) {
        let query = self.input.value().to_string();

        if query.is_empty() {
            // Undo the `clear_search_results()` pre-set (finding 7): Enter
            // in the search popup with an empty input cleared the list and
            // set loading + "Searching...", but this early return never
            // reaches the fetch paths that reset them — without this the
            // UI stranded on a fake spinner until the next search. The
            // list is empty here, so the idle status is the same one the
            // empty screen shows at startup (state.rs / App::run).
            *self.loading.write() = false;
            *self.status.write() = "Welcome! Press '/' to search for models".to_string();
            return;
        }

        *self.loading.write() = true;
        *self.error.write() = None;

        let models = self.models.clone();
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

        // Step 2: Fetch from API (if not cached). The session's shared
        // client (M4/B5) carries the token; no token plumbing here.
        let results = crate::api::fetch_models_filtered(
            &self.api_client,
            &query,
            sort_field,
            sort_direction,
            min_downloads,
            min_likes,
            100,
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

    /// Status-bar details for the selected FLARE revision card (the
    /// `show_quantization_details` precedent, plans/flare-models.md §3.4).
    /// The FULL commit pin
    /// lives here — the card shows only the short form.
    pub async fn show_flare_revision_details(&mut self) {
        let flare = self.flare_info.read();
        let Some(selected) = self.flare_list_state.selected() else {
            return;
        };
        let Some(revision) = flare.as_ref().and_then(|info| info.revisions.get(selected)) else {
            return;
        };
        let aliases = if revision.aliases.is_empty() {
            String::new()
        } else {
            format!(" ({})", revision.aliases.join(", "))
        };
        *self.status.write() = format!(
            "Revision: {}{} | Commit: {} | Size: {} | Files: {}",
            revision.revision,
            aliases,
            revision.target_commit,
            crate::fmt::size_full(crate::ui::render::flare_package_size(&revision.package,)),
            crate::ui::render::flare_package_files(&revision.package).len(),
        );
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

        // plans/flare-models.md §3.4 ownership rule: this spawn supersedes
        // every in-flight
        // FLARE scan (and any selection change supersedes it in turn).
        let epoch = self.begin_flare_scan();

        // Clone Arcs for background task
        let quantizations = self.quantizations.clone();
        let api_cache = self.api_cache.clone();
        let model_metadata = self.model_metadata.clone();
        let file_tree = self.file_tree.clone();
        let loading_quants = self.loading_quants.clone();
        let error = self.error.clone();
        let display_mode = self.display_mode.clone();
        let status = self.status.clone();
        // The FLARE publish slots (plans/flare-models.md §3.4) — one bundle
        // so the guarded
        // publish has a single home shared with the offline tests.
        let flare_shared = FlareScanShared::from_app(self);
        // The session's shared API client (M4/B5): cheap Arc clone, the
        // token rides in its default headers.
        let api_client = self.api_client.clone();

        // Spawn background task (non-blocking)
        tokio::spawn(async move {
            // Metadata cache-first via the shared get-or-fetch helper
            // (W4.11): read-lock fast path, unlocked fetch, Entry insert.
            // A failed fetch surfaces the historical error path and is
            // never cached — the next selection retries it.
            let metadata =
                match crate::models::ApiCache::get_or_fetch(&api_cache, &model_id, || {
                    fetch_model_metadata(&api_client, &model_id, crate::api::DEFAULT_REVISION)
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
            //
            // FLARE scan first (plans/flare-models.md §3.4): a confirmed
            // disposition publishes
            // Flare mode and RETURNS; any error or non-confirmation
            // degrades to the Standard/GGUF classification below (with a
            // status note on a failed scan) — never a dead end.
            let flare_confirmed = match flare_disposition(&metadata) {
                FlareDisposition::No => false,
                FlareDisposition::Yes => true,
                FlareDisposition::Probable => {
                    confirm_flare_probable(&api_client, &model_id, &metadata.siblings).await
                }
            };
            if flare_confirmed {
                let scanned = crate::models::ApiCache::get_or_fetch(&api_cache, &model_id, || {
                    fetch_flare_info(&api_client, &model_id)
                })
                .await;
                match scanned {
                    Ok(info) => {
                        flare_shared.publish(epoch, info);
                        return;
                    }
                    Err(e) => {
                        *status.write() =
                            format!("FLARE scan failed: {e} — showing file tree instead");
                        // fall through to the Standard/GGUF classification
                    }
                }
            }

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

        // Clear the FLARE scan result (plans/flare-models.md §3.4: cleared
        // wherever the
        // quant/tree state clears; `flare_platform` is SESSION-scoped and
        // stays) and invalidate any in-flight FLARE scan for the old
        // selection (ownership rule).
        *self.flare_info.write() = None;
        self.invalidate_flare_scans();

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
        // Shared API client (M4/B5) — see the load_quantizations clone.
        let api_client = self.api_client.clone();

        // Spawn background prefetch task (fire-and-forget)
        tokio::spawn(async move {
            for model_id in model_ids {
                // Metadata: cache-first via the shared helper (W4.11); a
                // failed fetch skips this model (never cached — retried
                // on a later prefetch).
                let metadata =
                    match crate::models::ApiCache::get_or_fetch(&api_cache, &model_id, || {
                        fetch_model_metadata(&api_client, &model_id, crate::api::DEFAULT_REVISION)
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

#[cfg(test)]
mod tests {
    //! Finding 7 regression: `clear_search_results()` pre-sets
    //! `loading = true` + "Searching..."; the empty-query early return in
    //! `search_models` must undo both, or pressing Enter in the search
    //! popup without typing strands the UI on a fake spinner forever.
    use super::*;
    use crate::paths::{ENV_CONFIG_DIR, ENV_MUTEX};

    /// Env guard restoring one variable on drop (same pattern as
    /// `mouse_tests.rs`; `App::new` reads the ambient config path).
    struct VarGuard {
        key: &'static str,
        saved: Option<std::ffi::OsString>,
    }

    impl VarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let saved = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, saved }
        }
    }

    impl Drop for VarGuard {
        fn drop(&mut self) {
            match self.saved.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    /// `App::new` is sync and lazy (no EventStream), so the TUI container
    /// is directly test-constructible (same fixture discipline as
    /// `downloads.rs` tests). Only the config-dir env needs redirecting
    /// here: the empty-query path never touches the network or registry.
    fn app_with_input(value: &str) -> (App, TempDir) {
        let tmp = TempDir::install();
        let mut app = App::new();
        app.input = tui_input::Input::default().with_value(value.to_string());
        (app, tmp)
    }

    struct TempDir {
        tmp: std::path::PathBuf,
        _g: VarGuard,
    }

    impl TempDir {
        fn install() -> Self {
            let tmp = std::env::temp_dir().join(format!("rhd-search-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            std::fs::create_dir_all(&tmp).expect("create temp config dir");
            Self {
                _g: VarGuard::set(ENV_CONFIG_DIR, &tmp.to_string_lossy()),
                tmp,
            }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }

    // ENV_MUTEX is a std mutex whose guard intentionally spans the awaited
    // body: it serializes env-var mutation across the WHOLE test
    // (single-threaded #[tokio::test] runtime, so the parked future never
    // blocks another ENV_MUTEX user). Pre-existing at v2.18.0 (main
    // 0ae3fc3); scoped here so the repo gate (clippy --all-targets -D
    // warnings) is green for feature work.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn empty_query_after_clear_search_results_resets_loading_and_status() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (mut app, _tmp) = app_with_input("");

        // The exact state the search popup's Enter handler leaves behind:
        // clear_search_results() pre-sets loading + "Searching...".
        app.clear_search_results();
        assert!(*app.loading.read(), "clear_search_results pre-sets");
        assert_eq!(*app.status.read(), "Searching...");

        app.search_models().await;

        assert!(!*app.loading.read(), "empty query must clear loading");
        let status = app.status.read().clone();
        assert_ne!(status, "Searching...", "empty query must reset status");
        // Neutral idle status, not an error or stale searching line.
        assert_eq!(status, "Welcome! Press '/' to search for models");
        assert!(app.models.read().is_empty(), "list stays cleared");
    }

    // ------------------------------------------------------------------
    // plans/flare-models.md §3.4 ownership rule: the reversed-completion
    // pin. A slow scan for
    // model A completing AFTER a fast selection change to B must not
    // publish over B's state — the publish guard is the epoch captured
    // at spawn, invalidated by every selection change.
    // ------------------------------------------------------------------

    /// Minimal one-revision scan value whose revision name identifies it.
    fn flare_info_for(revision: &str) -> crate::models::FlareRepoInfo {
        use crate::models::{
            FlareConfigFacts, FlarePackage, FlareRepoInfo, FlareRevision, ParsedPackageTokens,
            RefKind, RepoFile,
        };
        FlareRepoInfo {
            revisions: vec![FlareRevision {
                revision: revision.to_string(),
                aliases: Vec::new(),
                kind: RefKind::Branch,
                target_commit: format!("{:040}", 0),
                package: FlarePackage::Packed {
                    file: RepoFile {
                        rfilename: "pkg.safetensors".to_string(),
                        size: Some(1),
                        oid: None,
                        lfs: None,
                    },
                },
                parsed: ParsedPackageTokens::default(),
            }],
            config: FlareConfigFacts::default(),
            publisher: None,
            dropped_refs: Vec::new(),
        }
    }

    #[test]
    fn flare_publish_guard_pins_reversed_completion() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _tmp = TempDir::install();
        let mut app = App::new();
        let shared = FlareScanShared::from_app(&app);

        // Slow scan A starts; the user moves to B (every selection change
        // clears details, invalidating in-flight scans); scan B starts.
        let epoch_a = app.begin_flare_scan();
        app.clear_model_details();
        let epoch_b = app.begin_flare_scan();

        // Fast B completes FIRST: published — Flare mode, B's info, the
        // GGUF/Standard state cleared, spinner down.
        shared.publish(epoch_b, flare_info_for("gb202"));
        assert!(matches!(*app.display_mode.read(), ModelDisplayMode::Flare));
        assert_eq!(
            app.flare_info
                .read()
                .as_ref()
                .map(|info| info.revisions[0].revision.clone()),
            Some("gb202".to_string())
        );
        assert!(app.model_metadata.read().is_none());
        assert!(app.file_tree.read().is_none());
        assert!(app.quantizations.read().is_empty());
        assert!(!*app.loading_quants.read());

        // Slow A completes LAST: suppressed — B's publish survives intact.
        shared.publish(epoch_a, flare_info_for("gb10"));
        assert_eq!(
            app.flare_info
                .read()
                .as_ref()
                .map(|info| info.revisions[0].revision.clone()),
            Some("gb202".to_string()),
            "stale scan must not publish over the current selection's state"
        );

        // A CURRENT-epoch publish still goes through (the guard passes the
        // living scan, not just rejecting dead ones).
        let epoch_c = app.begin_flare_scan();
        shared.publish(epoch_c, flare_info_for("gb10"));
        assert_eq!(
            app.flare_info
                .read()
                .as_ref()
                .map(|info| info.revisions[0].revision.clone()),
            Some("gb10".to_string())
        );
    }
}
