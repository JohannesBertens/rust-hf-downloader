---
title: Agents Guide — Root
---

# AGENTS.md — AI Agent Guide for Rust HF Downloader

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
│   └── *_tests.rs + testutil.rs # per-subject cli test modules (M6/T1: args/cli_surface/events/hf_cache/help_snapshot/report/resolve_tests + shared fixtures in testutil); insta snapshots in src/cli/snapshots/
├── engine/           # Shared download engine (facade + private submodules, models/ precedent): mod.rs (EngineState, regrouped into bundles in M3 — queue: QueueAccounting { download_queue_totals, download_queue_items } / events: EventBus { status, verify, outcome tx+rx } / verification: the VerificationHub from verification.rs / + download_rx, download_progress, complete_downloads, download_registry — plus QueuedDownload and the field→bundle ownership table; the auth-status contract moved to models/engine.rs in M3), enqueue.rs (EngineState::enqueue + sealed EnqueuePolicy knob types + characterization tests), workers.rs (spawn_manager / spawn_verification_worker + ManagerHandle drain contract), bootstrap.rs (bootstrap + seed_registry_mirror)
├── models/           # Shared data types behind a facade (private submodules + pub use, W3.1): api.rs (HF DTOs), ui.rs (TUI enums + FileTreeNode), engine.rs (progress/queue/verification types incl. FileOutcome/VerifyOutcome + QueueTotals impl (renamed from QueueState in M3/U6) + the auth-status string contract, moved from engine/ in M3), options.rs (AppOptions — the config schema), cache.rs (ApiCache + aliases)
├── paths.rs          # App-path resolution only (config/registry/downloads; env override > portable mode > dirs defaults > temp) + sanitize. Never hardcode HOME or format! paths — route through this module. Hub-cache dir resolution + CACHEDIR.TAG moved to cache_layout (M6/C7); one-cycle pub-use shims remain
├── cache_layout.rs   # HuggingFace hub-cache owner (M6/C7): hub-cache dir resolution (hf_hub_cache: HF_HUB_CACHE/HF_HOME, huggingface_hub parity) + CACHEDIR.TAG + layout writer (v2.11.0): staging→blobs→snapshots atomic publish, relative symlinks, refs, sync lock (named cache_layout to disambiguate from cli/hf_cache/, the hf-cache command group)
├── patterns.rs       # Python-fnmatch parity glob matcher (`--include`/`--exclude`, `--for vllm` preset table)
├── update.rs         # Self-update (v2.10.0): latest.json manifest check, SHA256-verified asset download, self_replace swap; RHD_UPDATE_BASE override
├── config.rs         # Configuration persistence + apply_options (shared engine tuning)
├── api/              # HuggingFace API client behind a facade (W3.2): client.rs (fetch + pure filter_models/sort_models W3.3; every fetch fn takes the run's shared &reqwest::Client, M4/B5 — no per-request clients; recursive tree fetch propagates subdir errors, M4/B3; api_base() honors HF_ENDPOINT), quant.rs (GGUF classification + unified multipart parsing), tree.rs (file-tree building)
├── http_client.rs    # Authenticated HTTP requests (v0.9.5; M4/B5): build_client_with_token constructs the ONE shared client per run (token = default Authorization header; a non-representable token is an explicit ClientBuildError, never a silent unauthenticated downgrade) + get_with_optional_token thin GET wrapper
├── registry.rs       # Download metadata management + typed mutation ops behind a single writer (W2.4 + M1): register_pending (CLI pending seeder) / upsert_pending / upsert_metadata / mark_complete (Completion::{AlreadyExists, Downloaded} flavors) / mark_failed / mark_mismatch / delete_incomplete_by_urls (TUI delete) / purge_staging (hf-cache) — every write runs inside with_registry (process-global std-mutex single writer, leaf-only sync closures, returns the post-write snapshot for mirror replacement): load disk → mutate → ATOMIC save (same-dir temp + sync_all + rename-with-retry, Windows-aware) ; load_registry/save_registry are module-private (bypass sites cannot compile; tests/docs_guards.rs rejects hand-rolled registry-path fs writes); reads (read_registry) take no lock — the atomic save means a reader never sees a torn file; see registry.rs module docs)
├── download/         # Download transport with auth; returns FileOutcome (v0.9.5). Facade (mod.rs) holds start_download = prepare_download_paths / handle_existing_file / execute_download_with_retry phases (W5.1a), retry glue, and the global DOWNLOAD_CONFIG/RATE_LIMITER atomics; private chunked.rs (W3.8) holds download_chunked = probe_file_size + spawn_chunk_tasks/wait_for_chunks phases (W5.1b — tasks live in a JoinSet, M4/B4: first error aborts every sibling before the retry loop recreates the file, pinned deterministically in chunked::abort_tests), the per-chunk worker (bundled ChunkContext, W5.6), and chunk-size math. Error paths pinned by tests/download_failures.rs; the cross-chunk byte counter is an Arc<AtomicU64> (single-counter audit), the speed-pacing Instant+marker pair stays mutexed (compound)
├── rate_limiter.rs   # Token bucket rate limiter (v1.2.0)
├── verification.rs   # SHA256 verification worker (typed outcomes + idle signal)
├── fmt.rs           # Human-readable formatting primitives (W1.4): one wrapper per surface — eta_cli vs eta_hud, truncate_path_cli vs the middle-marker TUI variants, size_full vs HUD-compact bytes. The full-vs-HUD presentation split is DELIBERATE (do-not-unify contract pinned in fmt.rs:1-32); frozen-oracle tests pin every legacy string
├── utils.rs          # Exactly two helper families (slimmed W-final): streaming digests (stream_file_digest) + atomic rename with retry (sync + tokio twin). All human formatting lives in fmt.rs
└── ui/
    ├── mod.rs        # UI module exports
    ├── app/          # App module (W3.9: mod.rs — no app.rs indirection): run loop + draw + crossterm event loop + mouse click/scroll/hover handlers + submodule re-exports (App)
    │   ├── state.rs      # App state container and initialization
    │   ├── options.rs    # Options-dialog ownership (M5/U1): OptionsDialogState (cursor + live-edit flags + the two text-edit buffers) + OptionsFieldId/Kind + the OPTIONS_FIELDS 16-row table — render/options_popup.rs is a pure consumer
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
        ├── options_popup.rs # 16-field options dialog renderer — pure consumer since M5/U1 (dialog state + field table live in ui/app/options.rs, imported from there)
        ├── toolbar.rs     # filter & sort toolbar, hit areas, version badge
        └── *_tests.rs     # snapshot_tests / hud_tests / style_size_tests / tests — snaps in render/snapshots/
```

### Frontends share one engine (v2.13.2)

Every registry mutation goes through the typed ops in `registry.rs`
(W2.4, M1): `register_pending`, `upsert_pending`, `upsert_metadata`,
`mark_complete` (one fn taking a `Completion::{AlreadyExists, Downloaded}`
flavor — the two former `mark_complete`/`mark_complete_with_url` ops
merged), `mark_failed`, `mark_mismatch`, plus the M1 bulk ops
`delete_incomplete_by_urls` (TUI delete flow) and `purge_staging`
(hf-cache sync). Every op runs inside `registry::with_registry` — the
process-global single writer (`REGISTRY_WRITE`, a std mutex): load the
on-DISK registry → mutate → **atomic** save (same-dir temp file +
`sync_all()` + rename-with-retry) → return the post-write snapshot.
Callers that keep an engine mirror replace it from the returned snapshot
AFTER the writer releases (never patch the mirror under the lock);
`mark_mismatch`'s surgical mirror patch stays at its caller
(`verification::mark_mismatch_mirror`), run immediately after the op;
`mark_complete` patches the caller's complete-downloads mirror after the
save regardless of its outcome. Closures passed to `with_registry` are
**leaf-only** (no nested registry ops — a debug assertion fires; the std
mutex is non-reentrant) and must not `.await` (ops are sync; brief file
IO under the lock is accepted). `load_registry`/`save_registry` are
module-private to `registry.rs` — a new inline load-modify-save bypass
site cannot compile, and a hand-rolled `fs::write`/`File::create`
against the registry path fails
`tests/docs_guards.rs::registry_disk_writes_confined_to_registry_module`.
Reads (`registry::read_registry`) take no lock: the atomic save means a
concurrent reader sees the complete pre- or post-write file, never a
torn one. In-process lost updates are FIXED (M1, R4 sign-off: every
writer's update survives — the two-writer tests in
`registry::registry_tests` assert all-writers-win); cross-process safety
remains deferred (docs/DEFERRED.md#registry-cross-process-lock). Do not
reintroduce inline load-modify-save sequences at call sites — the
byte-level behavior of every op is pinned by the golden fixtures in
`registry::registry_tests`.

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
transaction (registry bookkeeping per policy →
`queue.download_queue_totals.add` → `queue.download_queue_items` mirror →
`download_tx` sends → failed-send rollback). The per-frontend divergences are explicit `EnqueuePolicy`
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
- **Filter State**: `src/ui/app/filters.rs` — `FilterState` (W4.5) is the single home of the live filter/sort values (`sort_field`, `sort_direction`, `min_downloads`, `min_likes`) and their cycle/step mutation rules; `App` holds it via `App.filters` (`src/ui/app/state.rs` — only `focused_filter_field`, the focus state, lives on App). The persisted *defaults* are `default_sort_*`/`default_min_*` on `AppOptions` (`src/models/options.rs`, `src/config.rs`), seeded into FilterState at startup and written back by `App::save_filter_settings`
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
- `options_dialog: OptionsDialogState` (owned by `ui/app/options.rs` since M5/U1) — transient options-dialog UI state (cursor row + live-edit flags + the two directory/token text-edit buffers); never serialized (AppOptions is pure config schema)

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
- Uses `try_lock()` for tokio Mutexes during render to prevent deadlocks (`ui::app::state::snapshot_in_place` helper + `RenderCache` on App: refresh cache when free, render cached snapshot when held)
- Uses `parking_lot::RwLock` which doesn't have poisoning (no `.unwrap()` needed)
- Cached render fields (App.render_cache) provide fallback when locks unavailable
- Mouse handler is synchronous to avoid blocking issues

### Mutex Lock Ordering (Critical for Deadlock Prevention)

The hierarchy below governs **blocking `.lock().await` acquisitions** on the
tokio `Arc<Mutex<..>>` fields of `EngineState` (plus the shared `RateLimiter`
state). NEVER hold a higher-numbered lock while acquiring a lower-numbered
lock. `try_lock()` access is deadlock-safe by construction — a miss skips
that read/tick instead of blocking — so non-blocking consumers (the UI's
render snapshot, the CLI runner's receiver drain in `cli/run.rs::poll_once`)
take locks in any order, each guard scoped to its own statement, never
nested.

Since M3 the engine's shared state is grouped into bundles
(`EngineState.queue` = `QueueAccounting`, `EngineState.events` = `EventBus`,
`EngineState.verification` = the `VerificationHub` owned by
`src/verification.rs`, plus four top-level fields). The bundle grouping is
stated once here and holds **per inner lock**: every member keeps its OWN
`Arc<Mutex<..>>` — no lock was merged, added, or re-scoped by the grouping —
and the numbered levels below apply to the inner locks exactly as they did
to the flat fields. This section is kept in sync with the struct definitions
by `tests/docs_guards.rs::lock_hierarchy_documents_every_engine_mutex_field`,
which derives the field set from the bundle struct bodies in
`src/engine/mod.rs` + `src/verification.rs` — a new mutex/atomic field fails
that test until it is documented here.

<!-- lock-hierarchy:begin -->
Lock Hierarchy (acquire in this order — the ordering constrains **blocking**
`.lock().await` acquisitions; scoped `try_lock` drains such as the receiver
tier below are exempt because a guard is released before anything else is
acquired). Bundle homes are noted per level; the grouping itself carries no
ordering — only the inner locks do.

1. `download_rx` (`Arc<Mutex<mpsc::UnboundedReceiver<QueuedDownload>>>` — EngineState top level; the WORK channel's receiver)
2. `download_queue_totals` (`Arc<Mutex<QueueTotals>>`) and `download_queue_items` (`Arc<Mutex<Vec<QueueItemSummary>>>`) — both in EngineState.queue (QueueAccounting): the totals counters and the HUD item mirror; acquire separately, never nested with each other (the rule is stated once, as the `QueueAccounting::remove_started` method)
3. `download_progress` (`Arc<Mutex<Option<DownloadProgress>>>` — EngineState top level)
4. `complete_downloads` (`Arc<Mutex<CompleteDownloads>>` — EngineState top level)
5. `queue` (`Arc<Mutex<Vec<VerificationQueueItem>>>` — EngineState.verification, the VerificationHub)
6. `size` (`Arc<AtomicUsize>` — VerificationHub) — lock-free atomic counter, carries no lock level
7. `progress` (`Arc<Mutex<Vec<VerificationProgress>>>` — VerificationHub)
8. `download_registry` (`Arc<Mutex<DownloadRegistry>>` — EngineState top level; the VerificationHub's `registry_mirror` is the SAME Arc shared into the hub, so this one level governs both names — and because a tokio Mutex is not re-entrant, NEVER acquire both names in one scope: holding one while awaiting the other self-deadlocks the task)
9. RateLimiter state (`Arc<Mutex<RateLimiterState>>`) — consolidated single lock, outside EngineState
10. Receiver tier — `status_rx`, `verify_rx`, `outcome_rx` (`Arc<Mutex<mpsc::UnboundedReceiver<..>>>`, all in EngineState.events, the EventBus; the matching `status_tx`/`verify_tx`/`outcome_tx` senders are plain channel endpoints and carry no lock): order-free under try_lock, drained one lock per scope with the guard released immediately (UI render snapshot; CLI `poll_once` drains status_rx → outcome_rx → the hub's `progress` → verify_rx via try_lock); never hold one receiver lock while blocking on another
11. `in_flight` (`Arc<AtomicUsize>` — VerificationHub) — lock-free atomic counter, carries no lock level
<!-- lock-hierarchy:end -->

Also lock-free, no hierarchy level: the VerificationHub's `results`
(`VerificationResultCounters` — session-lifetime ok/failed counters, each an
`Arc<AtomicUsize>`).

Also registry-internal, a TERMINAL LEAF below every engine lock:
`REGISTRY_WRITE` (the single-writer `std::sync::Mutex` inside
`src/registry.rs`, M1). It is not an `EngineState` field and carries no
level number, but it is NOT exempt from ordering — it has the strictest
position: it MAY be acquired while holding engine locks (callers do), and
while holding it NOTHING else may be acquired and no `.await` may happen
(closures are sync and leaf-only; mirror patches happen after
`with_registry` returns, and reads take no lock at all).

Key Rules:
- The ordering rule applies to BLOCKING `.lock().await` acquisitions: ALWAYS acquire them in the order above
- `try_lock()` never blocks, so it cannot join a deadlock cycle — use it for non-blocking access (UI rendering, receiver drains); ordering within a try_lock pass is free
- Release locks before acquiring locks from the same level if needed
- NEVER hold a lock across an await point unless absolutely necessary
- When receiving from a channel wrapped in Mutex, lock only for the recv() call

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

    // Now safe to acquire level 2 — both QueueAccounting locks, each taken
    // in its own scope, never nested with each other (in production this
    // pair is the QueueAccounting::remove_started method)
    {
        let mut totals = download_queue_totals.lock().await;  // Lock level 2
        totals.remove(1, total_size);
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
    let mut totals = download_queue_totals.lock().await;  // Lock level 2
    // DEADLOCK: If another task holds level 2 and needs level 1
}
```