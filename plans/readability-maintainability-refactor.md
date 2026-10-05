# Readability & Maintainability Refactor Plan

**Status:** reviewed — revised per three parallel plan audits (behavior-preservation: *sound after amendments*; coverage: *gaps found*; executability: *needs restructuring*; all accepted amendments folded in below; conditional code facts re-verified against HEAD)
**Date:** 2026-10-05 · **Base:** branch `refactor/readability-maintainability-review` @ `5286e0b` (v2.13.1)
**Sources:** four parallel fresh-context code reviews (architecture, pipeline internals, UI module, CLI + cross-cutting style), parent-verified against HEAD; then three parallel plan reviews (Opus soundness, glm-5.3 completeness, gemini executability).

## 1. Goals and contract

**Goal:** improve readability and maintainability only. **All observable functionality must remain exactly as is** — same CLI flags and exit codes, byte-identical NDJSON event streams, byte-identical TUI rendering, identical config/registry file formats, identical download/cache/update behavior *including timing-sensitive ordering* (event order, exit-code paths).

**Non-goals:** no new features, no dependency changes, no public API changes (crate is bin-only — verified: no `lib.rs`, no `[lib]` target), no async-runtime redesign, no behavior fixes even for real defects (§8), no big-bang rewrite.

**Declared invisible deltas** (cannot be avoided by restructuring; accepted, not observable through the app's own outputs):
- `Debug` representation of restructured types (W1.2 tuple→struct, W1.9 typed errors, W2.3, W3.x moves) changes in logs/panic messages if any type is ever `{:?}`-printed — grep before each merge; none known today.
- Insta `.snap` **headers** (`source:`/`expression:` lines) change on test moves; the "zero snapshot content changes" rule covers the snapshot **body** only.

**W4.1 help ordering — zero-delta target:** clap `#[command(flatten)]` with explicit `display_order` (or contiguous placement, since the shared flags are contiguous in both structs) preserves `--help` order exactly. Help snapshots (H2) are the enforcement; "review the diff" alone was rejected by review as non-enforcement. Fallback if clap still reorders: drop W4.1 — it is isolated.

## 2. Safety net

### 2.1 Harness hardening — lands FIRST, in PR1 alongside P0

| ID | Guard | Protects |
|----|-------|----------|
| H1 | **Characterization-first mandate**: every N→1 consolidation (W1.3, W1.4, W1.7, W2.1, W2.2, W2.5, W3.7, W4.2–W4.7, W4.11) lands a tests-first commit: old implementation kept as test oracle + differential/table tests over the divergent inputs, plus a **per-site divergence table in the PR description** — every divergence becomes an explicit parameter or a §8 entry. Never silently "picks one". | all consolidations |
| H2 | `--help` snapshots for every subcommand (`render_long_help()` + `Command::debug_assert()`). | W4.1 |
| H3 | Exit-code matrix test (per outcome mix: success, partial failure, auth-required, interrupted, usage). | W4.3, W2.6 |
| H4 | Human-output goldens (TTY + non-TTY stderr/stdout) for download, update, and error paths — today only NDJSON is snapshot-covered. | W1.4, §8.10 |
| H5 | TUI snapshots extended: style-aware assertions (focused/unfocused/selected/error cell styles — verify how snapshots serialize styles first) + a terminal-size matrix incl. odd sizes and the HUD threshold. | W2.5, W3.4, W4.8, W4.10 |
| H6 | Full error-code NDJSON golden: table test serializing **all 26** `Event::Error` codes + both `FileStatus` values against their literal strings; auth-failure + multi-file partial-failure NDJSON snapshots. | W1.3, W2.6, W4.3 |
| H7 | Concurrency checklist per PR: lock acquisition order matches the (corrected) AGENTS.md hierarchy; no guard lifetime extended (W0.8 rule); channel-sender drop order unchanged (W2.3, W4.3); two-writer registry interleaving test (W2.4); run e2e in a loop (20×) to surface new flakes. | W0.8, W2.x, W4.11, W5.6 |
| H8 | Insta CI gating: `INSTA_UPDATE=no`, `cargo insta test --unreferenced=reject`, fail on `.snap.new`, plus a diff check that moved `.snap` **bodies** are byte-identical (headers excluded). | W3.4, W3.5 |

Baseline (already in place): `cargo test` (unit + integration incl. `tests/cli_download.rs` live e2e), insta snapshots (UI + CLI NDJSON), `cargo clippy`/`fmt` clean. Every commit compiles and tests green (bisect-friendly).

### 2.2 Recurring rules

- Manual TUI smoke **per PR** (not per phase), checklist extended: mouse use, resize, auth/token entry, option change during in-flight download (must take live effect — guards W5.5), hf-cache flow, network-error state, quit-while-downloading (guards W2.3 drop order).
- Windows-only behavior (W1.1 reserved names, rename retry) has no Linux CI: keep per-site behavior unless a Windows job exists (§8 note).
- Local-server tests may assert request counts / `Authorization` presence where network behavior is adjacent (W4.11).

## 3. Cross-cutting principles

- **R1** one home per concept (flatten clap structs; `args.rs:158-198` vs `270-305`).
- **R2** wire-format strings are enums/constants, never inline literals (26 literal `Event::Error.code` sites: hf_cache_cmd 13, download_cmd 11, search_cmd 2).
- **R3** one env-var read per concern — *but* W4.2 must leave `AppOptions::default`'s `HF_TOKEN` read (`models.rs:420`) intact; removing it is §8.8, a behavior change requiring sign-off.
- **R4** no `#[allow(dead_code)]` without a dated reason naming the consumer (`patterns.rs:28,40,66,81`; also `api.rs:561` — folded into W0.1; `models.rs` `InputMode::Editing`'s undated "potential future use" — deleted by W0.2).
- **R5** `//!` module docs on every top-level module.
- **R6** visibility `pub(super)` inside `cli`/`ui` unless re-exported (bin-only crate — verified safe).
- **R7** cross-command machinery lives in a neutral module (`download_cmd::monitor`/`RunTally` consumed by `hf_cache_cmd`).
- **R8** human-output formatting exists once in `report.rs`-owned helpers (the `update_cmd` violation moves to §8.10 — unifying it changes stderr bytes).
- **R9** comments describe contracts, not history; docs match code at all times (W0.4 + per-commit doc updates, §8).

## 4. Workstreams

Source tags: **[A]** architecture, **[P]** pipeline, **[U]** UI, **[C]** CLI/style review. Risk = behavior-preservation risk after mitigation. Sub-letters (a/b/c) = commit-level splits.

### Phase 0 — Hygiene + harness (PR1)

| ID | Change | Evidence | Risk |
|----|--------|----------|------|
| W0.1 | Delete stale `allow(dead_code)` attrs: `patterns.rs:28,40,66,81` (hf-cache sync shipped; production-wired at `hf_cache_cmd.rs:127-169`) **and** `api.rs:561` (`is_quantization_directory` — test-only, gated properly by W3.2). | [C] verified | None |
| W0.2 | Delete dead code: `tree_json` (`tests/cli_download.rs:106`); `InputMode::Editing` + `handle_editing_mode_input` (verified unreachable — every assignment sets `Normal`; enum has no serde derives, no `as u8` casts — verified); write-only `App.mouse_position` (only write `app.rs:452`). | [C][U] verified | None |
| W0.3 | **Docs truth pass (all four AGENTS.md + README + CONTRIBUTING)**: root `AGENTS.md` lock hierarchy names nonexistent locks (`download_queue_size`/`download_queue_bytes`; actual: `download_queue: Arc<Mutex<QueueState>>` + `download_queue_items`) and stale module map (~48/709/460 vs actual 606/1049/801); `src/ui/AGENTS.md:35-36` (`render_progress_bars` deleted, "14 fields" vs 16); `src/ui/app/AGENTS.md:47` (tuple lacks `revision`); `README.md:715-742` module tree; `CONTRIBUTING.md:130,143-144` (the :144 pointer is already wrong today). Drop numeric line counts everywhere. | [A][C] verified | None |
| W0.4 | Delete two orphaned `.snap` files (`…download_progress_with_chunks`, `…verification_progress_bar`) — test fns verified gone. | [U] verified | None |
| W0.5 | `//!` module docs on all bare top-level modules (`models.rs`, `utils.rs`, `config.rs`, `registry.rs`, `api.rs`, `download.rs`, `verification.rs`, `http_client.rs`, `rate_limiter.rs`). | [C] | None |
| W0.6 | Drop the 8 `#[cfg(test)]` use-walls in `cli/mod.rs:42-65`; `tests.rs` imports `super::args::…` directly. | [C] | None |
| W0.7 | Visibility standardization (bin-only crate — verified no lib/tests consumers); remove `get_config_path`/`get_registry_path` pass-throughs. | [C] | Low |
| W0.8 | Remove **18** `futures::executor::block_on` wrappers around parking_lot RwLock reads (`models.rs` ×4, `events.rs` ×11, `downloads.rs` ×3 — all verified `futures::executor`, no tokio flavor). **Rule: clone-in-one-expression, no bound guards** — `block_on(async { x.read().clone() })` drops the guard at expression end; a `let g = …` rewrite would extend hold time and touch the lock hierarchy. | [U] verified | Low |
| W0.9 | Hoist `purge_staging_registry_entries()` out of the per-item publish loop (`hf_cache_cmd.rs`). **Executed in amended form (worker guard caught a plan error: no post-run sweep exists; the first in-loop purge is load-bearing)**: single guarded call immediately before the publish loop (post-drain sweep point preserved), comment documents the sweep semantics; see §10. | [C] amended | Low |
| H1–H8 | Harness hardening (§2.1). | plan audits | n/a |

### Phase 1 — Shared foundations (PR2)

| ID | Change | Evidence | Risk |
|----|--------|----------|------|
| W1.1 | Path-security consolidation: move `sanitize_path_component`, `validate_and_sanitize_path`, `nearest_existing_ancestor`, Windows reserved names from `download.rs:46-202` → `paths::sanitize` (tests move along; error strings preserved verbatim). `hf_cache::validate_relative_path` stays a thin policy wrapper documenting its deliberately stricter rules. | [A][P] | Low |
| W1.2 | `DownloadMessage` 7-tuple → `QueuedDownload` struct (`engine.rs:35-41`). **6 send sites** (download_cmd.rs:205; downloads.rs:312, 394, 583, 742; hf_cache_cmd.rs:674 — three are multi-line sends; original "3 sites" count was wrong, corrected by audit). Conversion commits map each site positionally; H1 oracle tests first. | [A][P] corrected | Low |
| W1.3 | `Event::error(code, message)` constructor (26 sites) + `FileStatus` enum (`events.rs:74`). **H6 golden table first** (all codes × all statuses serialized against literal strings; `Some("")` vs `None` under `skip_serializing_if` pinned). | [C] | Low (Med before H6) |
| W1.4 | Formatting module `src/fmt.rs`: primitives + per-surface wrappers pinning exact strings (HUD `~1h05m` vs CLI `1h05m`, middle-`~` vs tail-`…` truncation — distinct by design, documented). **Old helpers kept as oracles**; boundary tables (rounding `{:.1}` vs `{:.2}`, 1000-vs-1024 thresholds, ETA at speed 0, char-vs-unicode-width truncation) + new==old property tests before any deletion. Commit split: W1.4a create+test, W1.4b migrate callers. | [C][U] | **High** (mitigated to Med by oracles) |
| W1.5 | Atomic-rename helper `utils::atomic_rename_with_retry` extracted from `download.rs:544-557` **with per-site policy** — hf_cache adopts it only with a policy that reproduces its current semantics exactly (no new retries); **update.rs is out of scope** (audit correction: `update.rs:461-466 swap()` delegates to the `self_replace` crate — nothing to unify; redirecting it would change swap semantics). | [P] corrected | Low |
| W1.6 | Streaming digest helper; `git_blob_sha1` (`hf_cache.rs:80-86`) whole-file read → streaming. Known-vector tests (empty → `e69de29…`, `"hello\n"` → `ce01362…`, >buffer file); stat-length-vs-bytes-read mismatch errors exactly like today's whole-file path. | [P] | Med |
| W1.7 | One multipart parser replacing the 3 divergent ones (`api.rs:523, 575+595-630, 728`), `LazyLock` regexes. **Differential corpus test (new == old_i per site i) precedes the merge**; any divergence → site wrapper or §8, never a silent pick. | [P] | Med (gated) |
| W1.8 | **Descoped**: signature `Option<&str>` only. The stderr warning (silent-header-drop) is §8.7 — removed from this item. Shared `reqwest::Client` pooling is §8.11 (auth freshness, per-site timeouts, HTTP/2 multiplexing across 8 chunk workers — network-visible). | [P] rescoped | Low |
| W1.9 | Typed errors for `register_pending` + `validate_and_sanitize_path`. Display golden per variant; no `#[source]` unless today's string already included the cause (changes anyhow `{:#}` chains); grep for `e.contains(`/string-matching on these errors first. | [P] | Med |

### Phase 2 — Engine facade & state (PR3; internal order is load-bearing)

Order (audited): **W2.3 → W2.2 → W2.4 → W2.1 → W4.4**. W2.1 before W2.3 would force throwaway `engine_state()` clones at 127 field accesses; W2.4 after W2.1 would re-refactor what enqueue just baked in; W4.4 pulled forward from P4 so `downloads.rs` is rewritten once, not churned across three PRs.

| ID | Change | Evidence | Risk |
|----|--------|----------|------|
| W2.3 | `App` composes one `EngineState`. **W2.3a**: add `pub engine: EngineState`, init via `EngineState::new()`, keep the flattened fields temporarily (everything compiles). **W2.3b**: delete the 17 duplicate fields (`state.rs:41-67`), the inline channel wiring (`state.rs:120-131`), and `engine_state()` re-bundle (`state.rs:216-236`, verified cloning all 17 Arcs — no divergent state exists today); repath **127 field accesses** across 5 files (audited count; not "~5 edits"). **Explicit `self.engine.x` access — `Deref` is forbidden** (silent method resolution; fights W5.3). Drop-order check: quit-while-downloading in the smoke suite (field/drop order changes when senders close). | [A] verified | Med |
| W2.2 | `engine::bootstrap()` — CLI sites correct as cited (`download_cmd.rs:176-184`, `hf_cache_cmd.rs:637-645`); **TUI citation corrected**: the TUI never calls `EngineState::new` — it wires inline (`state.rs:120-131`, spawns `app.rs:39-40`, mirror seeded `downloads.rs:11-20`), which is exactly the divergence W2.3 removes; `App::new` is sync, so bootstrap offers a sync init path with spawns deferred to `App::run`. Divergence table (order of spawns vs `apply_options`) in PR. | [A] corrected | Med |
| W2.4 | Registry ops centralized. **W2.4a**: golden fixtures — full file bytes after each op sequence **and** op-semantics tests: disk-as-source-of-truth preserved (ops load-modify-save like today, never write from the mirror), mirror behavior on save failure pinned, two-writer interleaving test (H7). **W2.4b**: typed ops (`upsert_pending`, `mark_complete`, `mark_failed`, `mark_mismatch`) updating disk + mirror together; migrate the 5 inline sequences in `download.rs` + `verification.rs:187-198` op-by-op. **Explicitly does NOT make the write atomic** — that's §8.5; write ordering stays byte-identical. | [A][P] | Med |
| W2.1 | `EngineState::enqueue(files, policy)` — one home for the enqueue transaction (registry upsert → save → queue.add → send → items.push → failed-send rollback), today 6× (4 TUI flows + 2 CLI). **Per-site divergence table in the PR**; each divergence (TUI continue-on-invalid vs CLI abort-first; hf-cache's documented no-`register_pending` policy, `hf_cache_cmd.rs:629-635`) becomes an explicit `EnqueuePolicy` knob — the hf-cache sweep stays a named decision, not a comment. Failed-send rollback test per policy. Lock-order review (H7): the transaction acquires levels 2→3→4 — audit each legacy site's actual order first. | [A][U] verified | Med–High |
| W2.5 | Render snapshot helper `snapshot<T: Clone>(m, cache)` replacing 6 try-lock/fallback blocks in `draw()` (`app.rs:72-141`). **Diff the 6 blocks first** (fallback source may differ per field); keep variants if they differ. Fold `cached_*` into one `RenderCache`. | [A][U] | Med |
| W2.6 | Typed `EngineEvent::AuthRequired(model_id)` alongside the legacy `AUTH_ERROR:<id>` string (both emitted). Dual-emission guards: CLI must not serialize the new variant; each frontend consumes exactly one; auth-failure NDJSON snapshot (H6). String contract deleted only after both consumers migrate — final step, own commit. | [A][C] | Med |
| W4.4 | (Pulled forward from P4) Unify the three `confirm_*` flows on `enqueue` + `model_root()` helper — ~300 lines deleted. **Characterization first** (H1): App-level key-driven tests per flow asserting mode/focus/status/queue after confirm, since only manual smoke covers this today. Commit-per-flow breakdown like W2.4. | [U][A] | High (gated) |

### Phase 3 — Module splits & CLI runner (PR4; pure moves, no logic edits in the same commit)

Facade feasibility (audited): all four splits work; caveats folded in per item.

| ID | Change | Evidence | Risk |
|----|--------|----------|------|
| W3.1 | `models.rs` (605 lines, 32 types; ~450 production + tests) → `models/{api,ui,engine,options,cache}.rs`. **Submodules must be private** (`mod api; pub use api::*;`) — a public `models::api` would shadow `crate::api` under the four `use crate::models::*` globs in `ui/app/*` (verified present). Impl blocks (`QueueState:293`, `AppOptions:418`) move with their structs; `options.rs` imports `SortField`/`SortDirection` from `ui.rs`. | [A][C] | Low |
| W3.2 | **W3.2a**: delete/`#[cfg(test)]`-gate the five test-only shims (`api.rs:227, 335, 563, 575, 728`; zero production callers — verified; also removes the `AGENTS.md`/`src/AGENTS.md` references to `fetch_model_files`). **W3.2b**: pure move → `api/{client,quant,tree}.rs` with `api/mod.rs` re-export facade. | [A][P] | Low |
| W3.3 | Extract `filter_models`/`sort_models` from `fetch_models_filtered` (`api.rs:41-99`). Keep **stable** sort; tie-order characterization test first. | [P] | Low |
| Runner | **Consolidate W3.7 + W4.2 + W4.3 into one `cli/run.rs` extraction BEFORE W3.5** (audited: splitting hf_cache_cmd first would scatter the duplicated runner code into submodules): `RunTally`/`monitor`/`poll_once` (from `download_cmd.rs:222-491`; the lock-ordering contract documented at root `AGENTS.md:25` moves with them), `load_run_config()` (dedupes `download_cmd.rs:60-95` ≈ `hf_cache_cmd.rs:363-388` + partial variant at 946-952), `effective_revision()`, `resolve_token()` (4-site token precedence; **precedence matrix test first** — flag × env set/empty/unset × config per subcommand), `emit_metadata_error()`, `queue_run()` (engine bootstrap + queue accounting + send + drop — drop point stays byte-position-identical so manager-exit and final-event ordering are unchanged; exit-code matrix H3 first), `emit_run_failures()`, one `tally_outcome` shared by `apply_outcome_event` + `count_outcomes`. | [C][A] | Med (gated) |
| W3.5 | `hf_cache_cmd.rs` (981) → `cli/hf_cache/{selection,sync,path}.rs`. **Correction: no `.snap` moves** — hf_cache_cmd contains no tests; snapshots belong to `cli/tests.rs`. `cli/hf_cache/mod.rs` must re-export `select_sync_files`, `SelectionMode`, `SyncSelectionError`, `absolute_path` for `cli/tests.rs`. | [C] corrected | Low |
| W3.4 | `render.rs` (2873) → `render/{mod,models_list,standard,gguf,hud,popups,options_popup,toolbar}.rs`. **W3.4a**: tree helpers (`flatten_tree_for_navigation`, count/toggle) → `ui/tree.rs` (navigation model, not rendering). **W3.4b**: pure move + snapshot relocation. **Snapshot-path preservation trick (audited)**: declare the existing test modules (`snapshot_tests`, `hud_tests`, …) as direct child modules of `render/mod.rs` (e.g. `render/snapshot_tests.rs`) — module paths stay `crate::ui::render::snapshot_tests`, so **`.snap` filenames don't change**; only the directory moves to `src/ui/render/snapshots/`. **W3.4c** (separate commit, logic edit): `border_style`/`panel_list` dedup (4 verbatim copies verified at render.rs:113, 342, 467, 614) — only after H5 style-aware snapshots exist. HUD formatters live in `crate::fmt` (W1.4), not `render/hud.rs`. | [U] | Med |
| W3.6 | Rename `ui/app/models.rs` → `ui/app/search.rs` (collision with `crate::models`). Docs updated in same commit (§8). | [U] | Low |
| W3.7 | (Folded into Runner above; ID retained for traceability.) | | |
| W3.8 | **New (coverage gap)**: `download.rs` (~1020 production lines post-W1.1) → `download/{mod, manager, chunked}.rs` split so the §9 metric is met; lands after W5.1a decomposition has stabilized the contents (or exempt the file — decision point, see §9). | plan audits | Med |
| W3.9 | **New (coverage gap)**: `ui/app/events.rs` (1049 pure production lines) — post-W4.5/W4.6/W4.8 dedup, split `events/{keys,mouse,filters}.rs` if still > 800; decision point at PR5 time. | plan audits | Med |

### Phase 4 — Frontend duplication collapse (PR5 = TUI, PR6 = CLI)

| ID | Change | Evidence | Risk |
|----|--------|----------|------|
| W4.1 | `#[command(flatten)] RunOutputArgs` + `RateLimitArgs` (`args.rs:158-198` vs `270-305`), `display_order` set, H2 help snapshots enforce zero delta. | [C] | Low |
| W4.2 | (Folded into Runner; precedence matrix test is its gate.) | | |
| W4.3 | (Folded into Runner; exit-code matrix + drop-point identity are its gates.) | | |
| W4.5 | `app/filters.rs` `FilterState` (`cycle`/`step`/`refresh_request`): 4 sort-cycle matches, 6 step-table copies, 9 refresh tails — **including the two mouse sites** `app.rs:312, 446` (audited; plan originally implied keyboard-only). Divergence table first (step tables may differ intentionally; refresh tails may differ in selection reset). | [U] corrected | Med |
| W4.6 | `advance(state, len, forward)` — replaces **6** wrap-around fns (events.rs:614, 635, 656, 684, 950, 976 — audited; original "8" was wrong); model-list navigation is inline and out of scope. Table test per old fn (len 0/1/3 × None/first/last × direction) before replacement. | [U] corrected | Low |
| W4.7 | Options `FieldSpec` table: one `OPTIONS_FIELDS` drives the render vec (16 entries), the `selected_field < 15` bound (`events.rs:383`), and the 16-arm match (`events.rs:854`). **Reachability preservation is the gate**: if today's bound ≠ render count makes some field unreachable, the table must reproduce that exactly — silently fixing it is forbidden (latent defect → §8.13). Per-arm step/clamp/wrap semantics preserved. | [U] | High (gated) |
| W4.8 | `centered_rect`/`popup_shell` (5 popup prologues; integer-rounding variants must be diffed first); `FocusedPane::accepts_download()/accepts_verify()` predicates (OR-chains at `events.rs:42-58` + `app.rs:335-368`); `process_terminal_event` dedup (`app.rs:493-583`). Odd-size snapshots + hit-test unit tests (H5) — mouse handling has no snapshot coverage today. | [U] | Med |
| W4.9 | **Descoped**: selection-error unification (`ResolveError` vs `SyncSelectionError`) + `FileSpec::from(&RepoFile)` only. The `update_cmd` Reporter adoption **moves to §8.10** (audited: current hand-rolled line differs in bytes from Reporter's — `\r  downloading…` vs Reporter's shapes — human stderr would change with no snapshot covering it). | [C] rescoped | Low |
| W4.10 | HUD layout single-source. **Citation corrected**: the constant exists only at `app.rs:156`; the coupled shape lives in `render.rs:90-96` constraints. Threshold snapshots (height−1/threshold/threshold+1, H5) first. | [U] corrected | Med |
| W4.11 | `ApiCache::get_or_fetch` (4× pattern) + async-ify the 3 tokio-`Mutex` `block_on` fns. **Risk raised**: async conversion changes interleaving; must match each site's lock-held-during-fetch and error-caching behavior; local-server request-count assertions (§2.2). | [A] | Med |

### Phase 5 — Deeper structure (PR7; each item independently skippable)

| ID | Change | Evidence | Risk |
|----|--------|----------|------|
| W5.1 | **W5.1a**: decompose `start_download` (324 lines, `download.rs:204-528`) → `prepare_download_paths`/`handle_existing_file`/`execute_download_with_retry`. **W5.1b**: decompose `download_chunked` (286 lines, `:619-905`) → `probe_file_size` + spawn/wait phases. Guards: extracted `?` must not skip tail cleanup (temp-file removal, `mark_failed` paths) — failure-injection e2e first (500 mid-chunk, 416 on resume, size mismatch). JoinSet-abort stays §8.2. | [P] | Med |
| W5.2 | `RenderParams` (27 fields, 2 `&mut Vec` out-params, **3** construction sites — audited: app.rs:163, render.rs:2431, 2582) → grouped substructs; **preserve hit-rect push order** (first-match lookup). | [U] corrected | Med |
| W5.3 | `App` sub-struct grouping (`Filters`, `MouseState`, `RenderCache`, `EngineChannels`); drop-order smoke re-run. Optional sub-item (audited as serialization-neutral): move §8.9's serde-skipped transient fields out of `AppOptions` behind a TOML golden test. | [U] | Med |
| W5.4 | **Moved to §8.12** — the "observably identical" claim was unsound: even though the current queue is FIFO (`verification.rs:54 remove(0)` — verified, so an mpsc preserves order), end-of-run NDJSON ordering, HUD accounting, and pickup timing all shift. | [P] reclassified | — |
| W5.5 | `RuntimeConfig`, **split and rescoped**: W5.5a moves the three global statics (`DOWNLOAD_CONFIG` 11 atomics at `download.rs:570-599`, `RATE_LIMITER`, `VERIFICATION_CONFIG`) into one struct **carried as `Arc` shared mutability, not per-task snapshots** — snapshots would kill the live option effect on in-flight downloads (TUI edits rate/limits mid-run today; smoke-checklist item guards it). W5.5b (only if 5.5a lands cleanly) makes `apply_options` the constructor. Test save/restore today at `engine.rs:~305-315` (citation corrected) gets deleted with the statics. | [A] rescoped | High |
| W5.6 | Chunk-progress mutex → `AtomicU64` `fetch_add`. **Verify the mutex guards a single counter** (not bytes+timestamp pairs) before converting — if it guards compound state, keep the mutex and only bundle params into `ChunkContext` (which is sound regardless). | [P] | Low–Med |

## 5. Ordering, parallelism, effort

```
PR1: P0 + H1–H8              (hygiene + harness)
PR2: P1                       (W1.4a before W1.4b; W1.7/W1.3/W1.6 oracle-first)
PR3: P2                       W2.3a → W2.3b → W2.2 → W2.4a → W2.4b → W2.1 → W4.4 → (W2.5, W2.6 anytime after W2.3)
PR4: P3                       W3.1 → W3.2a/b → W3.3 → Runner(W3.7+W4.2+W4.3) → W3.5 → W3.4a → W3.4b → W3.4c → W3.6
PR5: P4-TUI                   (W4.5, W4.6, W4.7, W4.8, W4.10, W4.11)
PR6: P4-CLI                   (W4.1, W4.9)
PR7: P5                       (W5.6, W5.2, W5.3, W5.1a, W5.1b, W3.8, W3.9, [W5.5a, W5.5b last])
```

**Two parallel tracks are possible after PR3** (audited file-disjointness): Track A = CLI (Runner/W3.5/W4.1/W4.9), Track B = TUI (W3.4/W3.6/W4.5-W4.11). PR1/PR2 also split into disjoint hygiene/foundation pairs if staffing allows. The serial anchor is PR3.

**Effort (audited):** P0 S (1–2d) · P1 M (3–4d) · P2 L (4–6d; W2.3=127 sites, W2.1 lock-sensitive) · P3 L (4–5d; W3.4 alone multi-day) · P4 L (4–5d) · P5 L–XL (5–7d). **Total ≈ 21–29 person-days.**

## 6. Reviewer-convergence highlights

Independently found by ≥2 of 4 code reviews: enqueue/bootstrap duplication (A,U,C); `DownloadMessage` tuple (A,P); path-sanitization scatter (A,P); registry disk/mirror duality (A,P); `models.rs` grab-bag (A,C); formatting triplication (C,U); stale history comments (P,C,A). All three plan reviews independently flagged: the 800-line metric arithmetic, W1.8's smuggled behavior changes, and missing characterization tests for N→1 merges — all fixed above.

## 7. Clean areas (verified, intentionally untouched)

`paths.rs` resolution core, `registry.rs` persistence shape, `http_client.rs` focus, `cli/events.rs` schema, `hf_cache.rs` plan/publish split (production ~680 lines; file total 1326 is test-inflated), `cli/mod.rs` dispatch, the snapshot suite, `main.rs` (thin dispatch), `tests/cli_download.rs` (the safety net itself — splitting mid-refactor adds risk for no payoff).

## 8. Adjacent defects & deferred behavior changes (NOT in scope; each needs sign-off)

1. `api.rs:210-213` — `fetch_recursive_tree` silently swallows subdir fetch errors → truncated trees.
2. `download.rs:858-874` — chunk failure breaks the wait loop without aborting siblings (zombie workers); JoinSet fixes it.
3. `verification.rs:173` — `&expected_sha256[..16]` panics on malformed hash.
4. `rate_limiter.rs:101-105` — `rate == 0.0` enabled → `from_secs_f64(INFINITY)` panic.
5. `registry.rs:23-33` — non-atomic write; crash truncates; load silently resets. (W2.4 deliberately does not fix this.)
6. `engine.rs:169-171` — queue removal matches filename alone across models/revisions.
7. `http_client.rs:20` — invalid header silently dropped → unauthenticated request.
8. `models.rs:420` — `AppOptions::default()` reads `HF_TOKEN` (second env source beside `merge_token`).
9. `models.rs:~399-407` — transient UI state inside `AppOptions` (serde-skipped; structural move is W5.3-optional, the env read is #8).
10. `update_cmd.rs:158-164` — hand-rolled human progress line diverges from Reporter (adopting Reporter changes stderr bytes; needs H4 goldens first).
11. `reqwest::Client` per-request construction → shared pooled client (auth freshness, timeouts, multiplexing).
12. Verification worker busy-poll → channel (W5.4 reclassified; FIFO verified so ordering is preservable, but timing/NDJSON-tail effects need H6 multi-file ordering snapshots first).
13. Options-dialog reachability: if `selected_field < 15` bound ≠ 16 rendered fields today, some field is unreachable (W4.7 gate will surface it).

## 9. Acceptance criteria

- [ ] Every PR: full `cargo test` green, clippy/fmt clean, e2e loop (H7) clean, smoke checklist signed.
- [ ] Zero snapshot **body** changes (headers excluded by definition, §1); moved `.snap` bodies byte-identical (H8 check).
- [ ] `--help` byte-identical (H2) — W4.1's zero-delta target enforced, not reviewed.
- [ ] Exit-code matrix (H3) and precedence matrix green through Runner and W2.6.
- [ ] Registry goldens (W2.4a) green through W2.4b and after; two-writer test in suite.
- [ ] Size metric (audited definition): **no production module > 800 lines**, where "production" excludes `#[cfg(test)]` contents, `tests.rs` files, and `tests/` — this covers `cli/tests.rs` (1355) and `hf_cache.rs` (1326 total / ~680 prod, declared clean §7). Decision points if still over: W3.8 (download.rs) / W3.9 (events.rs) split, or a documented exemption in this file.
- [ ] Docs blast radius: all four AGENTS.md (root, `src/`, `src/ui/`, `src/ui/app/`) + `README.md:715-742` + `CONTRIBUTING.md:130,143-144` updated **in the same commits** as the moves/renames they describe; lock-ordering contract relocates with the Runner; §1's declared deltas stay accurate.
- [ ] Every item traces to a cited review finding; clean areas (§7) untouched.
- [ ] W5.5 live-option-effect smoke (change rate limit mid-download) green after 5.5a.

## 10. Execution log

All work lands as sequential commits on `refactor/readability-maintainability-review` (no PRs; single-branch diff for final review). Validation gate per slice: full `cargo test` (baseline 266/0), clippy clean, fmt clean.

| Commit | Items | Notes |
|---|---|---|
| 5e4b061 | W0.1, W0.6 | 4 stale attrs + api.rs:561 gate (`#[cfg(test)]`), 8 cli test use-walls removed |
| 8f875f4 | W0.9 (amended), W0.2, W0.4, W0.8 | W0.9 guard deviation: `if !plan.fetch.is_empty()` (no `items` binding in scope); 18/18 block_on sites were parking_lot, 1 tokio site left for W4.11; clippy zero warnings after tree_json removal |
| dab6cfb | W0.5, W0.7 | module docs on 9 bare modules; merge_token/apply_rate_limit_overrides/valid_model_id/truncate_path -> pub(super); get_config_path/get_registry_path pass-throughs removed |
| 0b24d69 | W0.3 | docs truth pass, 6 files; lock hierarchy rewritten to real locks (10 levels); found stale render.rs:1669 "14 fields" comment (fixed in W4.7 scope) |
| 602fe8e | H2, H6 | 7 help surfaces + debug_assert snapshotted; 26 construction sites = 13 distinct codes pinned byte-exact; +11 tests (277/0) |
| c910215 | H3, H4 | exit-code matrix (EXIT_OK/USAGE/FAILURE/AUTH; INTERRUPTED=pty GAP; UPDATE/CHECKSUM pinned by update_e2e); 7 human-output goldens + shared tests/common harness; regex dev-dep; +10 tests (287/0) |
| 7dc6129 | H5 | style_run signatures pin focus/hover/selection/popup styles (previously unpinned: TestBackend Display is symbols-only); 4-size matrix; HUD threshold clamp pinned; +11 tests (298/0) |

Group 1 (P0 + harness) complete at 298/0. Known residual: ~1% pre-existing e2e flake (unattributed; guard = name-and-rerun policy). Workers: local big-ai GLM-5.3-Flash (slices 1-6), remote zai glm-5.3 (H5 onward, after big-ai retirement).

**Group 2 (Phase 1 foundations) complete at 326/0:**
| Commit | Items | Notes |
|---|---|---|
| 7f48993 | W1.1 | paths::sanitize; 8 tests moved; importers repointed (engine.rs, ui/app/downloads.rs only — CLI/hf_cache importers were speculative); hf_cache wrapper documented |
| 2dc3ef9 | W1.5 | utils::atomic_rename_with_retry(retries, delay); download=4x100ms == old ATTEMPTS=5; hf_cache delegates retries=0 (byte-identical); update.rs untouched (self_replace) |
| c094dba | W1.2 | QueuedDownload struct; 7 sites incl. engine test; hf-cache slot-2=revision resolved from site comment; alias deleted; roundtrip test pins sha/token distinction |
| (2 commits) | W1.3, W1.8 | ErrorCode(13)+Event::error routed 26 sites; FileStatus enum (1 internal reader updated); DISCOVERY: dynamic codes beyond the 13 (unknown_revision, ambiguous, no_files_match, unknown_preset, empty_selection) stay String; error_with_available dead-but-tested; http_client Option<&str> x2 fns, 6 call sites |
| (2 commits) | W1.4 | fmt.rs: 12 helpers, 433 differential oracle comparisons, oracle algorithms inlined into tests; format_size/format_number remain as delegates (21/10 call sites); zero old-vs-old disagreements beyond documented surface split |
| fb3c597 | W1.6 | stream_file_digest core; verification.rs adopts; update.rs NOT (hashes network chunks while writing — different shape); git_blob_sha1 streaming + stat-len mismatch -> UnexpectedEof (documented amendment); hello\n vector corrected to ce013625030ba8dba906f756967f9e9ca394464a |
| 80ab5a7 | W1.7 | parse_multipart_info (string slicing, once_cell Lazy — repo rustc-1.75 idiom); 47-entry corpus differential test permanent; 9 site divergences PRESERVED via per-site rules (divergence table in commit msg) |
| 1333857 | W1.9 | PathError (8 variants, Display verbatim, no source); register_pending shares PathError; zero .contains( callers found; Display goldens |
| 12e67e0+bd3948b | W2.3 | App composes EngineState; 17 flattened fields deleted; 35 live self.<field> repaths (audit's 127 counted mentions); download_tx stays on App (engine returns it separately); engine_state() deleted; drop-order analysis in commit msg |
| b8ff168 | W2.2 | engine::bootstrap() + seed_registry_mirror() (single mirror-seed home); all 3 sites' seed semantics audited identical; +2 tests |
| 8a46154+859bd31 | W2.4 | registry typed ops (mark_complete(+url)/failed/mismatch/upsert_metadata/pending); goldens byte-identical through migration; two-writer test PINS the deferred lost-update race; save-failure mirror semantics pinned (RUST_HF_DOWNLOADER_DATA_DIR redirect; engine tests now share paths::ENV_MUTEX) |
| b733733+ed7d40c+375b4da | W2.1 | EngineState::enqueue + EnqueuePolicy (RegistryMode/QueueTiming/ItemsMirror/rollback; named per-site ctors); divergence table in commit; lock-order: no site ever nested locks; quant-flow sha256_map dead code removed; CLI register-after-bootstrap reorder proven unobservable; +8 tests |
| 6ce49ff+71cc1d9+9e9381c | W4.4 | App is now headless-test-constructible (event_stream moved to run()); 8 characterization tests pass byte-identical through unification; model_root/model_root_or + finish_enqueue + confirm_scoped_repository_download(RepoScope); -31 lines (W2.1 had absorbed the bulk); +8 tests |
