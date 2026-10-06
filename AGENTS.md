---
title: Agents Guide — Root
---

# AGENT.md - AI Agent Guide for Rust HF Downloader

This document provides AI agents with a comprehensive understanding of the Rust HF Downloader codebase, its architecture, and how to work with it.

## Project Overview

**Rust HF Downloader** is a Terminal User Interface (TUI) application written in Rust that allows users to search, browse, and download models from the HuggingFace model hub. It provides an interactive, keyboard-driven interface with vim-like controls and comprehensive download management.

### Modular Design 
The application follows a modular architecture with clear separation of concerns:

```
src/
├── main.rs           # Entry point: `download`/`search` subcommand → cli::run; no args → TUI
├── cli/              # One-shot CLI (`download` + `search` + `update` + `hf-cache` subcommands), split by section (v2.13.1):
│   ├── mod.rs        # Cli/Command clap roots, EXIT_* consts, run() dispatcher, re-exports
│   ├── args.rs       # All *Args structs + parse/merge helpers (parse_preset, merge_token, …); shared RunOutputArgs/RateLimitArgs flag blocks flattened into download + hf-cache sync (help order preserved)
│   ├── resolve.rs    # File resolution (FileSpec/Selector/parse_selector/resolve_files) — pure
│   ├── events.rs     # Stable additive-only NDJSON event schema (Event/OverallProgress)
│   ├── report.rs     # Reporter (human/NDJSON), --progress modes, progress-line formatters
│   ├── download_cmd.rs # run_download (config → resolve → enqueue → monitor → summary/exit)
│   ├── run.rs          # Cross-command runner (W3.7+W4.2+W4.3): RunTally + monitor/poll_once drain (lock ordering!), tally_outcome, load_run_config/resolve_run_token (the former also builds the run's ONE shared reqwest::Client, M4/B5 — a malformed token is an explicit auth failure there), effective_revision, queue_run, metadata-error + client-error + run-tail emissions
│   ├── search_cmd.rs # Query-only search (no engine)
│   ├── hf_cache/     # hf-cache group (private submodules + mod facade re-exporting selection/sync helpers): sync pipeline (selection, sync lock, publish) + path helper
│   ├── update_cmd.rs # Self-update subcommand (UpdateEvent NDJSON)
│   └── tests.rs      # cli::tests — insta snapshots in src/cli/snapshots/
├── engine/           # Shared download engine (facade + private submodules, models/ precedent): mod.rs (EngineState + QueuedDownload + auth-status contract), enqueue.rs (EngineState::enqueue + sealed EnqueuePolicy knob types + characterization tests), workers.rs (spawn_manager / spawn_verification_worker + ManagerHandle drain contract), bootstrap.rs (bootstrap + seed_registry_mirror)
├── models/           # Shared data types behind a facade (private submodules + pub use, W3.1): api.rs (HF DTOs), ui.rs (TUI enums + FileTreeNode), engine.rs (progress/queue/verification types incl. FileOutcome/VerifyOutcome + QueueState impl), options.rs (AppOptions — the config schema), cache.rs (ApiCache + aliases)
├── paths.rs          # Cross-platform path resolution (config/registry/downloads; env override > portable mode > dirs defaults > temp). Never hardcode HOME or format! paths — route through this module.
├── cache_layout.rs   # HuggingFace hub cache layout writer (v2.11.0): staging→blobs→snapshots atomic publish, relative symlinks, refs, sync lock (named cache_layout to disambiguate from cli/hf_cache/, the hf-cache command group)
├── patterns.rs       # Python-fnmatch parity glob matcher (`--include`/`--exclude`, `--for vllm` preset table)
├── update.rs         # Self-update (v2.10.0): latest.json manifest check, SHA256-verified asset download, self_replace swap; RHD_UPDATE_BASE override
├── config.rs         # Configuration persistence + apply_options (shared engine tuning)
├── api/              # HuggingFace API client behind a facade (W3.2): client.rs (fetch + pure filter_models/sort_models W3.3; every fetch fn takes the run's shared &reqwest::Client, M4/B5 — no per-request clients; recursive tree fetch propagates subdir errors, M4/B3; api_base() honors HF_ENDPOINT), quant.rs (GGUF classification + unified multipart parsing), tree.rs (file-tree building)
├── http_client.rs    # Authenticated HTTP requests (v0.9.5; M4/B5): build_client_with_token constructs the ONE shared client per run (token = default Authorization header; a non-representable token is an explicit ClientBuildError, never a silent unauthenticated downgrade) + get_with_optional_token thin GET wrapper
├── registry.rs       # Download metadata management + typed mutation ops (W2.4): register_pending (CLI pending seeder) / upsert_pending / upsert_metadata / mark_complete (Completion::{AlreadyExists, Downloaded} flavors) / mark_failed / mark_mismatch — every registry write routes through them (disk is source of truth: load disk → mutate → non-atomic save; pure disk ops — the mismatch engine-mirror patch lives at the verification caller; see registry.rs module docs)
├── download/         # Download transport with auth; returns FileOutcome (v0.9.5). Facade (mod.rs) holds start_download = prepare_download_paths / handle_existing_file / execute_download_with_retry phases (W5.1a), retry glue, and the global DOWNLOAD_CONFIG/RATE_LIMITER atomics; private chunked.rs (W3.8) holds download_chunked = probe_file_size + spawn_chunk_tasks/wait_for_chunks phases (W5.1b — tasks live in a JoinSet, M4/B4: first error aborts every sibling before the retry loop recreates the file, pinned deterministically in chunked::abort_tests), the per-chunk worker (bundled ChunkContext, W5.6), and chunk-size math. Error paths pinned by tests/download_failures.rs; the cross-chunk byte counter is an Arc<AtomicU64> (single-counter audit), the speed-pacing Instant+marker pair stays mutexed (compound)
├── rate_limiter.rs   # Token bucket rate limiter (v1.2.0)
├── verification.rs   # SHA256 verification worker (typed outcomes + idle signal)
├── utils.rs          # Exactly two helper families (slimmed W-final): streaming digests (stream_file_digest/sha256_file) + atomic rename with retry (sync + tokio twin). All human formatting lives in fmt.rs
└── ui/
    ├── mod.rs        # UI module exports
    ├── app/          # App module (W3.9: mod.rs — no app.rs indirection): run loop + draw + crossterm event loop + mouse click/scroll/hover handlers + submodule re-exports (App)
    │   ├── state.rs      # App state container and initialization
    │   ├── events/       # Keyboard dispatch (W3.9): mod.rs = on_key_event router + shared navigation/filter-value methods + W4.6 advance contract tests; keys.rs = normal-mode + popup key handlers
    │   ├── filters.rs    # FilterState: single home of filter/sort values + cycle/step mutation rules (W4.5)
    │   ├── search.rs     # Model browsing logic (search, details, quantizations)
    │   ├── downloads.rs  # Download management (trigger, confirm, resume/delete)
    │   └── verification.rs # Verification UI (manual verify action)
    ├── tree.rs       # File-tree navigation model (flatten_tree_for_navigation / toggle_node_expansion / count_tree_files), shared by render + app/events + app/downloads
    └── render/       # UI rendering functions (W3.4b: facade over one file per panel)
        ├── mod.rs         # render_ui shell + RenderParams + the snap_ui helper (insta names hang off this module)
        ├── models_list.rs # Results list item spans
        ├── standard.rs    # Standard mode: model metadata + file tree
        ├── gguf.rs        # GGUF mode: quantization groups + their files
        ├── hud.rs         # Activity HUD (row builders, column math, state glyphs)
        ├── popups.rs      # resume / search / download-path / auth-error overlays
        ├── options_popup.rs # 16-field options dialog
        ├── toolbar.rs     # filter & sort toolbar, hit areas, version badge
        └── *_tests.rs     # snapshot_tests / hud_tests / style_size_tests / tests — snaps in render/snapshots/
```

### Frontends share one engine (v2.13.2)

Every registry mutation goes through the typed ops in `registry.rs`
(W2.4): `register_pending`, `upsert_pending`, `upsert_metadata`,
`mark_complete` (one fn taking a `Completion::{AlreadyExists, Downloaded}`
flavor — the two former `mark_complete`/`mark_complete_with_url` ops
merged), `mark_failed`, `mark_mismatch`. Each op loads
the on-DISK registry, mutates, saves (non-atomic by design — §8.5); no
lock is held across the load-modify-save (the concurrent-writer
lost-update race is a known deferred defect, §8). `mark_complete`
patches the caller's mirror after the save regardless of its outcome;
`mark_mismatch` is pure disk ops — its engine-mirror patch lives at the
caller (`verification::mark_mismatch_mirror`), run immediately after the
op. Do not reintroduce inline load-modify-save
sequences at call sites — the byte-level behavior of every op is pinned
by the golden fixtures in `registry::registry_tests`.

The CLI frontends (`cli::run_download`, `cli::hf-cache sync`) bootstrap the
queue/download pipeline through `engine::bootstrap()` — fresh
`engine::EngineState` → on-disk registry loaded into the `download_registry`
mirror via `engine::seed_registry_mirror` → `engine::spawn_verification_worker`
→ `engine::spawn_manager`, in that order (`hf-cache sync` purges staging
registry entries immediately before it). The TUI's `App::new` is sync, so it
composes the same pieces: `EngineState::new()` at construction,
`engine::seed_registry_mirror` in the startup scan, the two spawns in
`App::run` — there is exactly ONE manager bootstrap (the v1 headless CLI was
removed in v2.0.0 because its duplicated copy drifted). Every queue handoff
— all four TUI download flows and both CLI frontends — goes through
`EngineState::enqueue(files, policy)`: the single home of the enqueue
transaction (registry bookkeeping per policy → `download_queue.add` →
`download_queue_items` mirror → `download_tx` sends → failed-send
rollback). The per-frontend divergences are explicit `EnqueuePolicy`
knobs — the fields are sealed; the five named constructors
(`tui_quant`/`tui_repository`/`tui_resume`/`cli_download`/`hf_cache_sync`)
are the only public API: `RegistryMode` (TUI mirror-registry upsert vs CLI
`register_pending` disk upsert vs the two no-write flavors
`AlreadyRecorded`/`StagingSweep`), `SendDiscipline`
(`Interactive`/`Resume`/`Batch` — queue-accounting timing, HUD-mirror
population, and rollback collapsed into the three correlated combinations
that actually occur), and `InvalidPolicy` (`ReportAndQueue` for the mirror
flavors, `AbortAll` for the disk flavor, `SkipValidation` for the no-write
flavors); user-facing status/error
strings stay at the call sites (`EnqueueOutcome`). The CLI signals
completion by dropping `download_tx` (manager join resolves) and then
waiting for `EngineState::verification_idle()`; per-file results stream over
the `outcome_tx`/`verify_tx` channels. `HF_ENDPOINT` overrides all HuggingFace
base URLs — required knowledge for integration tests and mirror users.
Check `README.md` for more information.

### CI & Release Builds (v2.8.0)
- `.github/workflows/release.yml` fires ONLY on `v*` tag pushes and has two
  phases: (1) a `build` matrix that compiles + tests every supported target
  and packages each binary with a STABLE asset name
  (`rust-hf-downloader-<target-triple>.tar.gz` / `.zip`); (2) a `release`
  job that generates `SHA256SUMS` and publishes the GitHub Release via
  `softprops/action-gh-release@v2` with the binaries, checksums, and the
  one-liner installers (`install.sh`, `install.ps1`) attached. No PR/push
  CI — zero CI load between releases.
- Targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (cross,
  gcc-aarch64-linux-gnu linker), `aarch64-apple-darwin`,
  `x86_64-apple-darwin` (cross from the arm64 runner),
  `x86_64-pc-windows-msvc`. Tests run only on the three native runners.
- Stable asset names are load-bearing: `releases/latest/download/<asset>` is
  a GitHub CDN redirect with no api.github.com rate limit, which is how the
  installers resolve "newest version" without the API (and without jq).
  Do NOT put the version number into asset file names.
- One-liner installers live at repo root (`install.sh`, `install.ps1`) and
  are attached to each release by CI. They are testable offline via
  `RHD_DOWNLOAD_BASE` (e.g. a local dir over `file://` or an http server).
  v2.9.0 behavior: they take over an existing `cargo install` copy in
  `$CARGO_HOME/bin` (in-place upgrade + `cargo uninstall` handoff so cargo's
  install records stay clean — never edit .crates.toml/.crates2.json by
  hand), prefer `$CARGO_HOME/bin` when it is on PATH (cargo-binstall
  convention, since rustup prepends it to PATH), and run a volta-style
  post-install shadow check that warns when another copy resolves first.
- The release job also emits `latest.json` (version + per-triple sha256,
  generated with jq from SHA256SUMS) — the manifest the `update`
  subcommand consumes via the same CDN redirect. All 5 triples must be
  present or the release job fails.
- Release flow: bump Cargo.toml (+lock) → changelog entries → merge PR → push
  `vX.Y.Z` tag → release with binaries + installers appears.
- macOS arm64 builds come from `macos-latest`; Intel macs are covered by a
  cross-compiled `x86_64-apple-darwin` build on the same runner.
- Gotcha: a tag whose workflow file exists only in the tagged commit (not on
  the default branch) may not trigger the run — tag a commit that is already
  on `main`.

### Filter & Sort System (v1.0.0)
- **Filter State**: `src/ui/app/state.rs` - sort_field, sort_direction, filter_min_*
- **Filter Logic**: `src/ui/app/events/` (keys.rs) - keyboard controls and presets; `src/ui/app/filters.rs` - filter state and mutation rules
- **Filter UI**: `src/ui/render/toolbar.rs` - toolbar rendering with focus highlighting
- **Filter API**: `src/api/client.rs` - fetch_models_filtered() with pure client-side filter_models/sort_models (W3.3)
- **Filter Config**: `src/config.rs` - default_sort_*, default_min_* persistence

### Mouse Integration System
The TUI supports full mouse interaction with panels and filter toolbar:

**State tracking** (`src/ui/app/state.rs`):
- `mouse: MouseState` (W5.3) bundles the mouse interaction state:
  - `areas: MouseAreas` — hit-rects of the last rendered frame: `panels: Vec<(FocusedPane, Rect)>` (clickable regions per panel) and `filters: Vec<(usize, Rect)>` (filter fields 0=sort, 1=downloads, 2=likes); written once per frame by `App::draw` from `RenderOutput::mouse`
  - `hovered_panel: Option<FocusedPane>` — currently hovered panel for border highlighting
  - `last_move: Instant` — hover-update throttle (~60fps)
- `options_dialog: OptionsDialogState` (§8.9) — transient options-dialog UI state (cursor row + live-edit flags); never serialized (AppOptions is pure config schema)

**Event handling** (`src/ui/app/mod.rs`):
- `handle_mouse_click(column, row)` - focus panel or cycle filter on click
- `handle_mouse_scroll(scroll_up, column, row)` - navigate panel or cycle filter on scroll
- `handle_filter_click(field_idx)` - cycle filter value forward on click
- `handle_filter_scroll(field_idx, scroll_up)` - cycle filter value bidirectionally
- `update_hover_state(column, row)` - update hovered panel for border effects
- Event coalescing: drains pending events, coalesces mouse moves into single hover update

**Rendering** (`src/ui/render/` — the render pass is pure; panels return their hit-rects):
- `RenderParams` groups inputs per consumer (`FocusCtx`, `ListCtx`, `GgufPanelContext`, `StandardPanelContext`, `FilterCtx`, `StatusCtx`); it no longer carries mouse out-params
- `render_ui` RETURNS a `RenderOutput { hud_strip, mouse: MouseAreas }`; `App::draw` stores `mouse.panels`/`mouse.filters` on `App` each frame
- Hit-testing is first-match over each list, so REGISTRATION ORDER is behavior: filter fields 0,1,2 in display order; Results list, then bottom panels left-to-right (GGUF: QuantizationGroups, QuantizationFiles; Standard: ModelMetadata, FileTree)
- `render_filter_toolbar()` returns the three field rects
- Border styles: `render/mod.rs::border_style` (single guard: yellow focused, cyan hovered, default otherwise); `render/mod.rs::panel_list` is the shared list-panel shape (title + border + selection highlight)

**Non-blocking design**:
- Uses `try_lock()` for tokio Mutexes during render to prevent deadlocks (`ui::app::state::snapshot` helper + `RenderCache` on App: refresh cache when free, render cached snapshot when held)
- Uses `parking_lot::RwLock` which doesn't have poisoning (no `.unwrap()` needed)
- Cached render fields (App.render_cache) provide fallback when locks unavailable
- Mouse handler is synchronous to avoid blocking issues

### Mutex Lock Ordering (Critical for Deadlock Prevention)

To prevent deadlocks, all async code must acquire locks in the following order. NEVER hold a higher-numbered lock while acquiring a lower-numbered lock.

```
Lock Hierarchy (acquire in this order):

1. download_rx (Arc<Mutex<mpsc::UnboundedReceiver<QueuedDownload>>>)
2. download_queue (Arc<Mutex<QueueState>>) and download_queue_items (Arc<Mutex<Vec<QueueItemSummary>>>) — the consolidated queue accounting (size + bytes in one QueueState) and the Vec<QueueItemSummary> HUD mirror; acquire separately, never nested with each other
3. download_progress (Arc<Mutex<Option<DownloadProgress>>>)
4. complete_downloads (Arc<Mutex<CompleteDownloads>>)
5. verification_queue (Arc<Mutex<Vec<VerificationQueueItem>>>)
6. verification_queue_size (Arc<AtomicUsize>) - lock-free atomic counter
7. verification_progress (Arc<Mutex<Vec<VerificationProgress>>>)
8. download_registry (Arc<Mutex<DownloadRegistry>>)
9. RateLimiter state (Arc<Mutex<RateLimiterState>>) - consolidated single lock
10. status_rx (Arc<Mutex<mpsc::UnboundedReceiver<String>>>)

Key Rules:
- ALWAYS acquire locks in the order above
- Release locks before acquiring locks from the same level if needed
- Use try_lock() for non-blocking access in UI rendering
- NEVER hold a lock across an await point unless absolutely necessary
- When receiving from a channel wrapped in Mutex, lock only for the recv() call
```

**Example - CORRECT pattern (download manager):**
```rust
// ✅ Lock only when receiving, release immediately
loop {
    let message = {
        let mut rx = download_rx.lock().await;  // Lock level 1
        match rx.recv().await {
            Some(msg) => msg,
            None => break,
        }
    }; // Lock level 1 released

    // Now safe to acquire level 2 — each lock taken in its own scope,
    // never nested with each other (same level)
    {
        let mut queue = download_queue.lock().await;  // Lock level 2
        queue.remove(1, total_size);
    } // Lock released
    {
        let mut items = download_queue_items.lock().await;  // Lock level 2
        // ... update the queue-items mirror
    } // Lock released
}
```

**Example - INCORRECT pattern (causes deadlock):**
```rust
// ❌ WRONG: Holding level 1 while acquiring level 2
let mut rx = download_rx.lock().await;  // Lock level 1
while let Some(msg) = rx.recv().await {
    let mut queue = download_queue.lock().await;  // Lock level 2
    // DEADLOCK: If another task holds level 2 and needs level 1
}
```