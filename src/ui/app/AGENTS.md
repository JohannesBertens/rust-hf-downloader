---
title: Agents Guide — src/ui/app
---

# Agents Guide (src/ui/app)

This submodule holds application state, event handling, and async orchestration for search, selection, download, and verification.

Files and roles
- state.rs
  • struct App: central state with Arc<RwLock>/Arc<Mutex> fields for lists, caches, queues, progress
  • RenderCache (W2.5): one struct field grouping the last-known-good snapshots of the engine's tokio::Mutex state; draw() refreshes each field via the `snapshot_in_place(m, cache)` helper when the lock is free and falls back to the cached value when held (verification_queue_bytes stays a derived variant — summed under the guard, never cloning the queue Vec per frame)
  • App::new is headless-safe (no EventStream field — the terminal event stream is constructed once at the top of App::run, after the caller's ratatui::init, and passed into handle_crossterm_events; crossterm's source eagerly opens a tty fd, so eager construction made App::new panic in test environments)
  • `engine: EngineState` owns the engine-side shared state (download/status/verify/outcome channels, queue/registry/progress Arcs, verification counters); App::new constructs it once via `EngineState::new()`; every TUI access goes through explicit `self.engine.<field>` reads (no Deref, no flattened mirrors)
  • `download_tx` is the only channel endpoint kept on App: the frontend-owned sender half of the engine's download queue (dropping it ends the manager loop once drained)
  • engine_state() snapshot method is gone — App::run passes `self.engine.clone()` directly to engine::spawn_verification_worker / spawn_manager
  • App::new loads options from config; seeds filter defaults; prepares channels through EngineState::new
  • the startup registry-mirror seed lives in engine::seed_registry_mirror (called by scan_incomplete_downloads) — the same helper engine::bootstrap runs for the CLI, so the seed convention is encoded in one place
  • App::sync_options_to_config maps AppOptions → global atomics (download & verification configs)
  • Display flags: needs_search_models, needs_load_quantizations to defer heavy work until after a frame draw
  • File tree state for Standard mode; display_mode is shared to switch GGUF vs Standard

- events/ (W3.9 split: mod.rs facade + private keys.rs)
  • App::on_key_event (mod.rs) → dispatch by PopupMode; routes to keys.rs handlers
  • keys.rs owns the per-context key maps: Normal mode ('/'-search, 'o'-options, 'd'-download, 'v'-verify, 'q'-quit, 's'/'S' sort, 'f'/'+/-'/'r' filters, presets 1/2/3/4, Tab/Left/Right focus, j/k/arrows navigation, Enter details) and the five popup handlers: Search, Options (with inline editing for directory/token), ResumeDownload, DownloadPath, AuthError; would_change_settings lives here too (preset-key helper)
  • mod.rs keeps the shared, non-keyboard-specific surface: navigation (models next/previous; quantization-group, quantization-file and file-tree cursors sharing one free fn advance(state, len, forward) (W4.6) — wrap-around both ends, unselected lists pick index 0 in both directions, len 0 no-op; the len×selection×direction tables in mod tests pin the contract), focus_pane/toggle_focus/toggle_quant_subfocus, file-tree expansion, modify_focused_filter/apply_filter_preset/save_filter_settings, and modify_option
  • 'd'/'v' key guards use FocusedPane::accepts_download()/accepts_verify() (defined next to the enum in models/ui.rs; pane sets pinned by unit test there)
  • Options dialog dispatch is id-keyed (W4.7): the cursor bound derives from the OPTIONS_FIELDS table length (16 rows → last index 15), Enter-edit matches OptionsFieldId::DefaultDirectory/HfToken, and modify_option matches the field ids with the per-field step/clamp/toggle bodies kept arm-by-arm (they differ per field); selected_field is serde-skipped so it can never exceed the table via stale config
  • Filter preset application and persistence (Ctrl+S saves as defaults)
  • Filter VALUE mutations only route through App.filters (ui/app/filters.rs); events/ owns key dispatch, status wording, and write order
  • Mouse handling is NOT in events/ — handle_mouse_*/hover live in ui/app/mod.rs next to the crossterm loop

- filters.rs (W4.5)
  • FilterState { sort_field, sort_direction, min_downloads, min_likes } — the single home of the four filter values and their mutation rules
  • cycle(field, forward) = mouse click/scroll wrap-around semantics; step(field, delta) = keyboard '+/-' clamped semantics; step(0, −1) toggles the sort DIRECTION (not a backward cycle) — the intentional divergences are documented in the module header divergence table
  • toggle_direction/reset/apply_preset/matches_preset; refresh_request() consumes the needs_refresh flag and gates the clear_search_results + needs_search_models tail at every call site
  • Step tables DOWNLOAD_STEPS / LIKE_STEPS live here; off-table values (e.g. config-loaded 42) resolve to index 0
  • Seeded from AppOptions::default_* via from_options; saved back by App::save_filter_settings — the config serde surface stays AppOptions (byte-identical config files)

- search.rs (search + model-detail loading; was models.rs before W3.6)
  • search_models: cache-first on ApiCache.searches; calls api::fetch_models_filtered; sets loading/status (this searches-map site keeps its inline Entry insert: SearchKey keying + per-path exact-match post-filtering and status wording make it a poor fit for the model-keyed helper)
  • show_model/quant/file_details: updates status/selection info lines
  • spawn_load_quantizations: loads metadata via ApiCache::get_or_fetch (W4.11 — one helper for the read-check → unlocked fetch → Entry-insert pattern; lock is never held across a fetch, failures are never cached); chooses mode:
      - GGUF → classify_quantizations(metadata.siblings) grouped by quant type; clear Standard state
      - Standard → build_file_tree from metadata.siblings; clear GGUF state
    Sets loading flags; uses display_mode to inform rendering; prefetch_adjacent_models debounced (async fn since W4.11 — the UI's last futures::executor::block_on site; called from the async App::run loop)
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
- engine.events.status_tx/rx: strings consumed by run loop to update status and popups; the auth line AUTH_ERROR:<model_id> is built and parsed only through models::{auth_status_message, parse_auth_status} (W2.6 contract, moved to models in M3) — one string contract shared with the human CLI's status_line
- engine.verification.queue (+ size, in_flight) and engine.verification.progress (the VerificationHub, M3): shared with the verification worker

Caching strategy
- ApiCache: metadata, quantizations, file trees, and search results by SearchKey (includes all filters)
- Model-keyed reads go through ApiCache::get_or_fetch (models/cache.rs, W4.11): read-lock fast path, unlocked fetch (concurrent duplicate fetches are allowed — dedup at the Entry insert, first completed fetch wins), failed fetches returned as Err and never cached (retried next call); semantics pinned by the concurrency tests in models/cache.rs
- Always check cache first; keep UI responsive and avoid repeated HTTP calls

Safety and correctness notes
- Always use paths::sanitize::validate_and_sanitize_path for any user-provided path/filename
- Keep selection indices consistent with list lengths; guard against empty vectors
- When toggling modes, clear the complementary state to avoid stale UI
- AUTH errors push a special message handled to show AuthError popup

Adding features safely
- New input actions → events/keys.rs (key maps) or events/mod.rs (shared navigation); update status messages and focused pane logic if needed
- New background operations → set a flag, spawn task, update Arc/RwLock fields, and clear loading flags
- Persisted options → add to AppOptions (models/options.rs), map in sync_options_to_config, render in options popup
