---
title: Agents Guide — src/
---

# Agents Guide (src/)

Purpose: equip coding agents to quickly understand how core modules interact so you can extend or modify behavior safely.

Key runtime: async TUI app orchestrating HuggingFace model search, browsing, downloading, and SHA256 verification.

Data flow (high level):
- UI events (src/ui/app/events/ (mod.rs dispatch + keys.rs handlers)) mutate App state (src/ui/app/state.rs)
- Searches call API (src/api/ (client.rs)) via HTTP client (src/http_client.rs)
- Model results and caches live in App state (ApiCache in src/models/cache.rs)
- Selecting a model loads GGUF quantizations or repository metadata/file tree
- Downloads (src/download/, W3.8: mod.rs facade + private chunked.rs) stream in parallel with progress; registry (src/registry.rs) persists metadata
- Verification worker (src/verification.rs) validates SHA256 post‑download

Threading/async:
- Tokio runtime; heavy tasks spawned from App; shared state via Arc<Mutex>/Arc<RwLock>
- Global runtime-tunable atomics in DownloadConfig and VERIFICATION_CONFIG

Auth model:
- Optional HF token; only set Authorization header when non-empty; read from config and passed to api/http_client and downloads.

Key modules

1) models/ facade (api/ui/engine/options/cache submodules)
- Core types: ModelInfo, ModelMetadata(+RepoFile/LfsInfo), FileTreeNode
- Quantization: QuantizationInfo, QuantizationGroup
- Download tracking: DownloadMetadata/Registry, DownloadStatus, ChunkProgress, DownloadProgress
- App/UI enums: PopupMode, FocusedPane, ModelDisplayMode
- Filter/sort: SortField, SortDirection, FilterPreset; ApiCache and SearchKey
- Default AppOptions: persisted config schema only (download/verification and filter settings; the options dialog's transient UI state — cursor row, live-edit flags — moved to ui/render/options_popup.rs::OptionsDialogState, resolved in docs/DEFERRED.md#options-dialog-transient-state, pinned by the TOML golden test in options.rs)

2) http_client.rs
- build_client_with_token(token, timeout) -> reqwest::Client (adds Bearer header only if token is Some(non-empty))
- get_with_optional_token(url, token) -> Response (unauthenticated if token empty/None)

3) api/ facade (client/quant/tree submodules)
- fetch_models_filtered(query, sort_field, sort_direction, min_downloads, min_likes, token)
  • API supports only descending reliably; client-side sorts for Name or Ascending
  • Client-side filters: min_downloads, min_likes
- fetch_model_metadata(model_id, token)
  • Enriches metadata.siblings with complete recursive tree (fetch_recursive_tree)
- build_file_tree(files: Vec<RepoFile>) -> FileTreeNode with sizes and sorted dirs-first
- has_gguf_files(metadata) -> bool (test-only since v2.4; classification replaced its call sites)
- classify_quantizations(files: &[RepoFile]) -> Vec<QuantizationGroup>
  • Pure classifier over the full recursive tree (issue #25): mmproj files get
    MMPROJ/MMPROJ-<quant> groups; quant-named ancestor dirs are inherited;
    unrecognized GGUFs land in OTHER (sorted last) instead of being dropped;
    groups sorted by total_size desc
  • One predicate everywhere: looks_like_quant_type (Q/IQ/TQ/MXFP + BF16/F16/FP16/FP32)
  (v2.4's `fetch_model_files` compat wrapper was deleted: no callers remained;
   call fetch_model_metadata + classify_quantizations directly)
- resolve_revision_sha(model_id, revision, token) -> Result<String> (revision commit SHA; unknown revision → 404)
- Helpers: extract_quantization_type, get_multipart_base_name, looks_like_quant_type
  • Test-only (`#[cfg(test)]`): has_gguf_files, is_quantization_directory,
    extract_quantization_type_from_dirname, parse_multipart_filename

4) config.rs
- load_config() -> AppOptions (reads crate::paths::read_config_path(); defaults on missing/unparseable file; env HF_TOKEN override lives in AppOptions::default)
- save_config(&AppOptions) (writes crate::paths::config_path())
- apply_options(&AppOptions) — maps persisted options onto the global DOWNLOAD_CONFIG/VERIFICATION_CONFIG atomics
  • Tests cover path and default load

5) registry.rs
- Persistence of DownloadRegistry at ~/models/hf-downloads.toml
- load_registry/save_registry, selectors for incomplete/complete
- Typed mutation ops (W2.4) — every registry write routes through them; register_pending (CLI pending seeder, moved from engine.rs so pending writes have one owner) sits next to the upsert_pending op it drives; mark_complete is one fn taking a Completion::{AlreadyExists (status flip only) | Downloaded (status + downloaded_size + url rewrite)} flavor (the two former mark_complete/mark_complete_with_url ops merged); byte-level behavior pinned by the goldens in registry/registry_tests.rs

6) download/ (W3.8: mod.rs facade + private chunked.rs — every crate::download::X path unchanged)
- start_download(DownloadParams) async orchestrates a safe, parallel, ranged GET download, in three W5.1a phases:
  • prepare_download_paths: validates/sanitizes paths; restarts if .incomplete exists; preserves subdirectories in filename
  • handle_existing_file: already-exists branch (registry mark_complete, verification queueing, progress clear)
  • execute_download_with_retry: retry loop — transient errors consume a retry and delete .incomplete; 401 → AuthRequired; terminal failure → mark_failed + .incomplete cleanup
- chunked.rs: download_chunked = probe_file_size (Range probe, /raw fallback on 404, Content-Range/Content-Length parse) + file prealloc + spawn_chunk_tasks/wait_for_chunks; renames .incomplete -> final on success; queues verification when enabled and hash known
  • W5.6: each chunk task takes one bundled ChunkContext (client, url, incomplete path, progress handles, span, pacing state — formerly a 12-arg fn); the cross-chunk `progress_downloaded` counter is an Arc<AtomicU64> (audited: single u64, no compound state — fetch_add per stream item, relaxed load for the speed snapshot; rendered progress flows through DownloadProgress under its own lock); the speed-pacing Instant + byte-marker pair stays mutexed (compound state: the window gate and the marker update move as one unit)
- Path security: paths::sanitize::{sanitize_path_component, validate_and_sanitize_path} (see 13)) — start_download applies them to user-supplied filenames; blocks traversal
- DownloadConfig (global atomics in mod.rs) controls chunking, retries, timeouts, and UI update cadence

7) verification.rs
- VERIFICATION_CONFIG (global atomics)
- verification_worker: processes VerificationQueueItems with concurrency limit
- verify_file: streams file, computes SHA256 with progress, updates registry to HashMismatch on mismatch
- queue_verification: append to queue and increment size

8) ui/ (see nested AGENTS.md for details)
- mod.rs: exports app and render modules, re-exports App, declares the private tree module
- render/: all UI drawing — mod.rs holds the render_ui shell + RenderParams and delegates to models_list / standard / gguf / hud / popups / options_popup / toolbar; panes for models, GGUF, standard metadata + file tree, status, popups, progress bars
- tree.rs: file-tree navigation model (flatten_tree_for_navigation, toggle_node_expansion, count_tree_files) shared by render and app/*
- app.rs: run loop; spawns verification worker and download manager; defers network loads to avoid blocking draws
- app/*: state, events, model and download flows

9) utils.rs — exactly two genuinely-generic families (final pass; formatters moved to fmt.rs)
- digest streaming: stream_file_digest(path, hasher, buffer_size, on_chunk) (progress-reporting read+hash loop), DIGEST_CHUNK
- atomic rename: atomic_rename_with_retry / atomic_rename_with_retry_async (retry policy is per-site: download 4 retries/100ms linear backoff; cache_layout: retries=0 single attempt)

10) fmt.rs — human-readable formatting primitives (W1.4), one wrapper per surface
- ETA: eta_cli (f64 seconds, "?" guard) vs eta_hud (u64, ~ prefix, zero-padded hour minutes); truncation: truncate_path_cli (tail, leading …) vs the middle-marker TUI variants (truncate_filename …, truncate_name_middle_hud ~); bytes: size_full "1.00 GB" vs size_hud-compact "1.0GB" (binary 1024 thresholds); number is deliberately decimal
- The full-vs-HUD presentation split is DELIBERATE — do not unify the variants (contract pinned in fmt.rs:1-32); the #[cfg(test)] oracle module holds frozen copies of the old helper bodies and the table tests assert wrapper == oracle

11) cli/ — one-shot CLI surface (v2.3.0+, split into a directory)
- `download` + `search` + `update` + `hf-cache` subcommands (clap derive); reuses engine::bootstrap
- Split by section: mod (Cli/Command/run), args, resolve, events, report, run (cross-command runner: RunTally/monitor/poll_once + load_run_config/queue_run/run-tail emissions), download_cmd, search_cmd, hf_cache/ (mod = dispatch + the shared `absolute_path` + the facade re-exports `cli/tests.rs` imports; selection = pure plans/hf-cache-sync.md §2.2 selector; sync = §5.2 sync pipeline; path = snapshot-path math), update_cmd, per-subject *_tests.rs test modules + testutil fixtures (M6/T1)
- Human reporter or JSON Lines (`--json`); documented exit-code table
- `--revision`, rate-limit flags; HF_ENDPOINT honored via api::api_base

12) engine/ — the single shared download pipeline bootstrap (v2.9.x; split from one engine.rs into a facade + private submodules, external crate::engine:: imports unchanged)
- mod.rs: EngineState bundle (+ QueuedDownload message type, auth-status string contract W2.6); submodule facade (pub use enqueue/workers/bootstrap)
- enqueue.rs: EngineState::enqueue(files, policy) (W2.1): the one enqueue transaction (registry bookkeeping per policy → queue.add → HUD mirror → sends → failed-send rollback); every divergence between the six legacy inline sites is an EnqueuePolicy knob — fields sealed, the five named constructors are the only public API (tui_quant/tui_repository/tui_resume/cli_download/hf_cache_sync): RegistryMode {AlreadyRecorded (TUI resume) | StagingSweep (hf-cache sync) | Mirror (TUI confirms) | Disk (CLI register_pending)}, SendDiscipline {Interactive | Resume | Batch} (queue timing + HUD-mirror shape + rollback, collapsed to the three correlated combos that occur), InvalidPolicy {ReportAndQueue (Mirror) | AbortAll (Disk) | SkipValidation (no-write flavors)}; outcome {sent, invalid[], aborted} feeds the call sites' own status/error strings; the 8 characterization tests live here
- workers.rs: spawn_manager (serial channel consumer → download::start_download, queue accounting) + spawn_verification_worker + ManagerHandle (drain-based join contract)
- bootstrap.rs: bootstrap() (state → registry-mirror seed → verification worker → manager) + seed_registry_mirror()
- CLI: run::queue_run (engine::bootstrap → EngineState::enqueue → drop the sender) in run_download + hf-cache sync (after purging staging registry entries); TUI: composes the same pieces (EngineState::new in App::new, seed_registry_mirror in the startup scan, both spawns in App::run) — never duplicate this logic

13) paths.rs — app-path resolution (v2.6.0) + path-security policy (paths::sanitize)
- Precedence: env overrides > portable mode (config.toml next to exe) > dirs
  defaults > temp fallback; never hardcode $HOME or format! paths elsewhere
- sanitize: per-component sanitization (traversal, control/Windows-illegal chars,
  reserved device names) + containment-checked validate_and_sanitize_path
- Hub-cache dir resolution + CACHEDIR.TAG moved to cache_layout (M6/C7);
  one-cycle pub-use shims remain here

13b) cache_layout.rs — all HuggingFace hub-cache knowledge (M6/C7 consolidated)
- Hub-cache dir resolution (hf_hub_cache: --cache-dir > HF_HUB_CACHE >
  HUGGINGFACE_HUB_CACHE > HF_HOME > platform default — huggingface_hub
  parity) + write_cachedir_tag backup-tool marker (moved from paths.rs)
- Layout writer (v2.11.0): staging→blobs→snapshots atomic publish, relative
  symlinks, refs, per-repo sync lock, staging cleanup

14) rate_limiter.rs — token-bucket limiter (v1.2.0)
- Global VERIFICATION/DownloadConfig atomics; single consolidated state lock

15) update.rs — self-update (v2.10.0)
- `update` subcommand backend: fetch latest.json manifest (RHD_UPDATE_BASE
  override), strict VersionTriple compare, platform asset by target triple
- SHA256-verified streamed download to a temp dir; tar.gz (unix) / zip
  (windows) single-member extraction; `self_replace` atomic swap
- Own reqwest client — must NEVER send the HF token

Common extension points
- Add new filters/sorts: update models::SortField/SortDirection, ui render toolbar, events handlers, and api::fetch_models_filtered
- New verification logic: modify verification.rs and AppOptions + config mapping and UI options
- Additional file types: extend api::looks_like_quant_type/classify_quantizations and Standard mode panels

Conventions & gotchas
- Never add Authorization unless token is present and non-empty
- All filesystem writes for downloads use .incomplete then atomic rename
- Always keep paths under user’s chosen base; use validate_and_sanitize_path
- Cache first (ApiCache) before re-fetching to keep UI snappy
- UI draws read many RwLocks; keep heavy work off hot render path (spawn tasks and set flags)

Quick map
- Search: ui/app/search.rs::search_models -> api::fetch_models_filtered
- Select model: ui/app/search.rs::spawn_load_quantizations -> api::{fetch_model_metadata, classify_quantizations, build_file_tree}
- Download: ui/app/downloads.rs::{trigger_download, confirm_download, confirm_repository_download} -> download::start_download
- Verify: verification::verification_worker auto-runs; queue via download completion

Testing & quality
- Unit tests: config.rs
- When changing public APIs or types, run: cargo fmt --check; cargo clippy -D warnings; cargo test
