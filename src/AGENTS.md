---
title: Agents Guide — src/
---

# Agents Guide (src/)

Purpose: equip coding agents to quickly understand how core modules interact so you can extend or modify behavior safely.

Key runtime: async TUI app orchestrating HuggingFace model search, browsing, downloading, and SHA256 verification.

Data flow (high level):
- UI events (src/ui/app/events.rs) mutate App state (src/ui/app/state.rs)
- Searches call API (src/api.rs) via HTTP client (src/http_client.rs)
- Model results and caches live in App state (ApiCache in src/models.rs)
- Selecting a model loads GGUF quantizations or repository metadata/file tree
- Downloads (src/download.rs) stream in parallel with progress; registry (src/registry.rs) persists metadata
- Verification worker (src/verification.rs) validates SHA256 post‑download

Threading/async:
- Tokio runtime; heavy tasks spawned from App; shared state via Arc<Mutex>/Arc<RwLock>
- Global runtime-tunable atomics in DownloadConfig and VERIFICATION_CONFIG

Auth model:
- Optional HF token; only set Authorization header when non-empty; read from config and passed to api/http_client and downloads.

Key modules

1) models.rs
- Core types: ModelInfo, ModelMetadata(+RepoFile/LfsInfo), FileTreeNode
- Quantization: QuantizationInfo, QuantizationGroup
- Download tracking: DownloadMetadata/Registry, DownloadStatus, ChunkProgress, DownloadProgress
- App/UI enums: PopupMode, InputMode, FocusedPane, ModelDisplayMode
- Filter/sort: SortField, SortDirection, FilterPreset; ApiCache and SearchKey
- Default AppOptions: runtime + persisted defaults for download/verification and filter settings

2) http_client.rs
- build_client_with_token(token, timeout) -> reqwest::Client (adds Bearer header only if token is Some(non-empty))
- get_with_optional_token(url, token) -> Response (unauthenticated if token empty/None)

3) api.rs
- fetch_models_filtered(query, sort_field, sort_direction, min_downloads, min_likes, token)
  • API supports only descending reliably; client-side sorts for Name or Ascending
  • Client-side filters: min_downloads, min_likes
- fetch_model_metadata(model_id, token)
  • Enriches metadata.siblings with complete recursive tree (fetch_recursive_tree)
- build_file_tree(files: Vec<RepoFile>) -> FileTreeNode with sizes and sorted dirs-first
- has_gguf_files(metadata) -> bool
- fetch_model_files(model_id, token) -> Vec<QuantizationGroup>
  • Detects single/multipart .gguf and quantization dirs; groups by type, sorts by total_size desc
- fetch_multipart_sha256s(model_id, filenames[], token) -> map filename -> Option<sha256>
- Helpers: extract_quantization_type, is_quantization_directory, parse_multipart_filename, get_multipart_base_name

4) config.rs
- get_config_path() -> ~/.config/jreb/config.toml
- load_config() -> AppOptions (with env HF_TOKEN override)
- save_config(&AppOptions)
  • Tests cover path and default load

5) registry.rs
- Persistence of DownloadRegistry at ~/models/hf-downloads.toml
- load_registry/save_registry, selectors for incomplete/complete

6) download.rs
- start_download(DownloadParams) async orchestrates a safe, parallel, ranged GET download:
  • Validates/sanitizes paths; restarts if .incomplete exists; preserves subdirectories in filename
  • HEAD via Range to get total size; falls back to /raw endpoint on 404
  • Preallocates file; spawns chunk workers limited by DOWNLOAD_CONFIG.concurrent_threads
  • Updates DownloadProgress and registry continuously; renames .incomplete -> final on success
  • Queues verification when enabled and hash known
- validate_and_sanitize_path(base_path, model_id, filename) -> PathBuf; blocks traversal
- DownloadConfig (global atomics) controls chunking, retries, timeouts, and UI update cadence

7) verification.rs
- VERIFICATION_CONFIG (global atomics)
- verification_worker: processes VerificationQueueItems with concurrency limit
- verify_file: streams file, computes SHA256 with progress, updates registry to HashMismatch on mismatch
- queue_verification: append to queue and increment size

8) ui/ (see nested AGENTS.md for details)
- mod.rs: exports app and render modules and App type re-export
- render.rs: all UI drawing; panes for models, GGUF, standard metadata + file tree, status, popups, progress bars
- app.rs: run loop; spawns verification worker and download manager; defers network loads to avoid blocking draws
- app/*: state, events, model and download flows

9) utils.rs
- format_number, format_size helpers for UI

Common extension points
- Add new filters/sorts: update models::SortField/SortDirection, ui render toolbar, events handlers, and api::fetch_models_filtered
- New verification logic: modify verification.rs and AppOptions + config mapping and UI options
- Additional file types: extend api::has_gguf_files/extract_quantization_type and Standard mode panels

Conventions & gotchas
- Never add Authorization unless token is present and non-empty
- All filesystem writes for downloads use .incomplete then atomic rename
- Always keep paths under user’s chosen base; use validate_and_sanitize_path
- Cache first (ApiCache) before re-fetching to keep UI snappy
- UI draws read many RwLocks; keep heavy work off hot render path (spawn tasks and set flags)

Quick map
- Search: ui/app/models.rs::search_models -> api::fetch_models_filtered
- Select model: ui/app/models.rs::spawn_load_quantizations -> api::{fetch_model_metadata, fetch_model_files, build_file_tree}
- Download: ui/app/downloads.rs::{trigger_download, confirm_download, confirm_repository_download} -> download::start_download
- Verify: verification::verification_worker auto-runs; queue via download completion

Testing & quality
- Unit tests: config.rs
- When changing public APIs or types, run: cargo fmt --check; cargo clippy -D warnings; cargo test
