---
title: Agents Guide — src/ui/app
---

# Agents Guide (src/ui/app)

This submodule holds application state, event handling, and async orchestration for search, selection, download, and verification.

Files and roles
- state.rs
  • struct App: central state with Arc<RwLock>/Arc<Mutex> fields for lists, caches, queues, progress
  • App::new loads options from config; seeds filter defaults; prepares channels (download/status)
  • App::sync_options_to_config maps AppOptions → global atomics (download & verification configs)
  • Display flags: needs_search_models, needs_load_quantizations to defer heavy work until after a frame draw
  • File tree state for Standard mode; display_mode is shared to switch GGUF vs Standard

- events.rs
  • App::on_key_event → dispatch by PopupMode and InputMode
  • Normal mode keys:
    - '/' open Search popup; 'o' Options; 'd' Download; 'v' Verify (on selection); 'q' Quit
    - 's' cycle SortField; 'S' (Shift+s) toggle sort direction
    - 'f' focus next filter field; '+'/'-' modify focused filter; 'r' reset
    - Presets 1/2/3/4 → NoFilters/Popular/HighlyRated/Recent
    - Tab toggles pane focus; Left/Right switches quant subfocus
    - Enter: show details or toggle depending on pane (incl. file tree expansion)
  • Popup handlers: Search, Options (with inline editing for directory/token), ResumeDownload, DownloadPath, AuthError
  • Navigation helpers for models, quantizations, files, file tree
  • Filter preset application and persistence (Ctrl+S saves as defaults)

- models.rs (UI models logic)
  • search_models: cache-first on ApiCache.searches; calls api::fetch_models_filtered; sets loading/status
  • show_model/quant/file_details: updates status/selection info lines
  • spawn_load_quantizations: loads metadata (cache-first); chooses mode:
      - GGUF → fetch_model_files grouped by quant type; clear Standard state
      - Standard → build_file_tree from metadata.siblings; clear GGUF state
    Sets loading flags; uses display_mode to inform rendering; prefetch_adjacent_models debounced
  • clear_search_results/clear_model_details give immediate UI feedback

- downloads.rs
  • scan_incomplete_downloads: populates popup, complete map, and status
  • trigger_download: decides scope based on focused pane (group/file/repo)
  • confirm_download: validates paths, populates registry entries, fetches SHA256 map (multipart), queues N downloads
  • resume/delete incomplete downloads operate on registry + filesystem
  • confirm_repository_download: non-GGUF repo case; preserves folder structure under base/author/model

Important queues and channels
- download_tx/rx: (model_id, filename, base_path, expected_sha256, hf_token)
- status_tx/rx: strings consumed by run loop to update status and popups (e.g., AUTH_ERROR:<model_id>)
- verification_queue(+size) and verification_progress: shared with verification worker

Caching strategy
- ApiCache: metadata, quantizations, file trees, and search results by SearchKey (includes all filters)
- Always check cache first; keep UI responsive and avoid repeated HTTP calls

Safety and correctness notes
- Always use download::validate_and_sanitize_path for any user-provided path/filename
- Keep selection indices consistent with list lengths; guard against empty vectors
- When toggling modes, clear the complementary state to avoid stale UI
- AUTH errors push a special message handled to show AuthError popup

Adding features safely
- New input actions → events.rs; update status messages and focused pane logic if needed
- New background operations → set a flag, spawn task, update Arc/RwLock fields, and clear loading flags
- Persisted options → add to AppOptions (models.rs), map in sync_options_to_config, render in options popup
