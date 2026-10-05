---
title: Agents Guide — src/ui/app
---

# Agents Guide (src/ui/app)

This submodule holds application state, event handling, and async orchestration for search, selection, download, and verification.

Files and roles
- state.rs
  • struct App: central state with Arc<RwLock>/Arc<Mutex> fields for lists, caches, queues, progress
  • RenderCache (W2.5): one struct field grouping the last-known-good snapshots of the engine's tokio::Mutex state; draw() refreshes each field via the `snapshot(m, cache)` helper when the lock is free and falls back to the cached value when held (verification_queue_bytes stays a derived variant — summed under the guard, never cloning the queue Vec per frame)
  • App::new is headless-safe (no EventStream field — the terminal event stream is constructed once at the top of App::run, after the caller's ratatui::init, and passed into handle_crossterm_events; crossterm's source eagerly opens a tty fd, so eager construction made App::new panic in test environments)
  • `engine: EngineState` owns the engine-side shared state (download/status/verify/outcome channels, queue/registry/progress Arcs, verification counters); App::new constructs it once via `EngineState::new()`; every TUI access goes through explicit `self.engine.<field>` reads (no Deref, no flattened mirrors)
  • `download_tx` is the only channel endpoint kept on App: the frontend-owned sender half of the engine's download queue (dropping it ends the manager loop once drained)
  • engine_state() snapshot method is gone — App::run passes `self.engine.clone()` directly to engine::spawn_verification_worker / spawn_manager
  • App::new loads options from config; seeds filter defaults; prepares channels through EngineState::new
  • the startup registry-mirror seed lives in engine::seed_registry_mirror (called by scan_incomplete_downloads) — the same helper engine::bootstrap runs for the CLI, so the seed convention is encoded in one place
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
  • scan_incomplete_downloads: seeds the engine registry mirror via engine::seed_registry_mirror (one disk read, snapshot reused), populates popup, complete map, and status
  • trigger_download: decides scope based on focused pane (group/file/repo)
  • confirm_download: validates paths, fetches multipart SHA256s (the failure warning is observable), then queues through EngineState::enqueue with EnqueuePolicy::tui_quant — registry bookkeeping, queue accounting, HUD mirror, and failed-send rollback live in the engine; per-file user messages stay at this call site
  • resume/delete incomplete downloads: resume re-queues through engine.enqueue (EnqueuePolicy::tui_resume — no registry writes, queue accounted after sends); delete operates on registry + filesystem
  • confirm_repository_download / confirm_tree_download are thin entry points over one shared pipeline, confirm_scoped_repository_download(RepoScope) (W4.4): gather siblings per scope → model_root → payload → EngineState::enqueue (EnqueuePolicy::tui_repository) → shared tail. RepoScope is the only divergence (selection predicate + empty-selection/success wording); everything else is shared
  • shared helpers: `model_root(base, model_id)` / `model_root_or(base, id, fallback)` = base/author/model for a two-part id (fallback otherwise) — do not re-inline the split/join; `App::finish_enqueue(outcome, success, failure)` = the post-enqueue tail (invalid-filename errors + success/failure string) every confirm flow ends with
  • the flows' observable behavior (status/error strings, queue accounting, HUD items order, registry entries, channel messages, popup clear via on_key_event) is pinned byte-for-byte by the characterization tests in downloads.rs `#[cfg(test)]` — behavior-preserving refactors must keep them unchanged

Important queues and channels (all on `app.engine` except download_tx)
- download_tx (on App): sends QueuedDownload { model_id, revision, filename, base_path, expected_sha256, hf_token, total_size } into the engine queue
- engine.status_tx/rx: strings consumed by run loop to update status and popups; the auth line AUTH_ERROR:<model_id> is built and parsed only through engine::{auth_status_message, parse_auth_status} (W2.6) — one string contract shared with the human CLI's status_line
- engine.verification_queue(+size) and engine.verification_progress: shared with verification worker

Caching strategy
- ApiCache: metadata, quantizations, file trees, and search results by SearchKey (includes all filters)
- Always check cache first; keep UI responsive and avoid repeated HTTP calls

Safety and correctness notes
- Always use paths::sanitize::validate_and_sanitize_path for any user-provided path/filename
- Keep selection indices consistent with list lengths; guard against empty vectors
- When toggling modes, clear the complementary state to avoid stale UI
- AUTH errors push a special message handled to show AuthError popup

Adding features safely
- New input actions → events.rs; update status messages and focused pane logic if needed
- New background operations → set a flag, spawn task, update Arc/RwLock fields, and clear loading flags
- Persisted options → add to AppOptions (models.rs), map in sync_options_to_config, render in options popup
