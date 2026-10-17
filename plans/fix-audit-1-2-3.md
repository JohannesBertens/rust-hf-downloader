# Plan: Fix audit findings 1, 2, 3 (ponytail-audit round)

**Status:** proposed (feature branch `feature/fix-audit-1-2-3`)

**Source:** `ponytail-audit` report on this repo (2026-10-17). Three
findings, ordered by severity, all small and independent. This plan fixes
all three in one PR.

| # | Finding | File(s) | Kind |
|---|---------|---------|------|
| 1 | Symlink inside the download folder sends files outside it | `src/engine/enqueue.rs`, `src/download/mod.rs`, `src/paths.rs` | security / path containment |
| 2 | A hand-edited config with `0` (or reversed) tuning values hangs or panics the engine | `src/config.rs`, `src/download/chunked.rs` | robustness / availability |
| 3 | One keypress can crash the TUI if a filter-field index ever exceeds 2 | `src/ui/app/events/keys.rs`, `src/ui/app/mod.rs` | crash robustness |

---

## 1. Root cause and design

### Fix 1 — path containment

Two compounding sites (the audit's "the same rule applied differently in
two places"):

**1a. `EngineState::enqueue` Mirror arm still queues files that fail path
validation** (`src/engine/enqueue.rs`).

The TUI confirm flows (quant, repository, tree) run every queued file
through `paths::sanitize::validate_and_sanitize_path` against the **raw**
user base — the strong check that catches an intermediate symlink escape
(`~/models/org` → elsewhere). But the Mirror arm only *reports* the failure
(`EnqueueOutcome::invalid`) and **still sends the file** to the download
channel. The write-time defense (`prepare_download_paths`) then receives a
`base_path` already pre-built as `base/author/model` and canonicalizes it
*through* the symlink, so it cannot detect the escape. Net effect: the TUI
says "invalid path" and downloads it anyway, outside the chosen base.

The CLI is already safe (`RegistryMode::Disk` → `register_pending` aborts
on the first invalid file before anything is queued).

**Design 1a:** in the Mirror arm, a file whose path validation fails is
reported via `outcome.invalid` and is **not** queued, not counted in queue
accounting, not sent, and not pushed to the HUD mirror. Concretely, the
enqueue transaction builds a `queueable: Vec<QueuedDownload>` containing
only files that passed validation (Mirror arm) or all files (NoWrites and
Disk arms), and the queue accounting / HUD mirror / sends / failed-send
rollback all operate on `queueable` instead of `files`. `sent` then counts
only files actually handed to the channel, and `finish_enqueue`'s
`sent > 0` success test degrades correctly when every file is invalid.

This is a deliberate behavior change to a byte-pinned transaction: the
`enqueue_mirror_…_still_queues_them` characterization test and the
`EnqueueOutcome::invalid` / `RegistryMode::Mirror` / `finish_enqueue`
doc-comment contract all flip to "reported, not queued", and the matching
line in `AGENTS.md` is updated.

**1b. `prepare_download_paths` only checks the file's final parent**
(`src/download/mod.rs`).

The write-time containment check canonicalizes only `final_path.parent()`
and only when that parent already exists. A *deep* symlinked subdir whose
child directories do not exist yet (e.g. filename `Q4_K_M/sub/file.gguf`
where `…/model/Q4_K_M` is a symlink to elsewhere and `sub` is not created
yet) escapes the check and the file lands outside the base. `sanitize::
validate_and_sanitize_path` already implements the robust rule — walk up to
the **first existing ancestor**, canonicalize it, and require it to be
under the canonical base (or an ancestor of the base).

**Design 1b:** make `paths::sanitize::nearest_existing_ancestor`
`pub(crate)` and reuse it in `prepare_download_paths` in place of the
final-parent-only check, with the same containment predicate. This closes
the deep-subdir gap for every download flow, including `tui_resume` and
`hf-cache sync`, which deliberately skip enqueue-time validation. No
behavior change for non-symlinked paths (first existing ancestor is under
the base), so no pinned output changes.

### Fix 2 — config tuning values are applied unbounded

`apply_options` (`src/config.rs`) copies every numeric `AppOptions` field
into the engine atomics with **no bounds check** (the options screen clamps,
but a hand-edited `config.toml` is not). Concrete breakages, all reachable
from a hand-edited config:

| Config value | Engine result |
|---|---|
| `concurrent_threads = 0` | `Semaphore::new(0)` → every chunk task blocks forever → download/CLI hangs, no timeout |
| `num_chunks = 0` | `file_size / 0` → **panic** in `calculate_chunk_size` (manager task dies; CLI exits 0 having downloaded nothing — silent success) |
| `min_chunk_size > max_chunk_size` | `Ord::clamp` **panics** ("min > max") |
| `min_chunk_size = 0` and `max_chunk_size = 0` | `chunk_size = 0` → `div_ceil(0)` **panic** |
| `concurrent_verifications = 0` | verification semaphore has 0 permits → worker never completes → `verification_idle()` never true → CLI hangs in the post-download drain |
| `verification_update_interval = 0` | `iteration % 0` → **panic** inside the hashing thread |
| `download_timeout_secs = 0` | every request times out immediately → downloads always fail |

This is the same class as the already-fixed
`docs/DEFERRED.md#rate-limiter-zero-rate`: the CLI flag path validates, the
config-file path does not.

**Design 2:** add a `normalize_engine_options(&mut AppOptions)` step at the
top of `apply_options` (it already takes a shared table; the normalization
runs once, before the table and before the rate-limiter spawn, on a clone —
the persisted config keeps the user's raw values). Safety floors, chosen to
prevent hang/panic only and never to silently override legitimate values:

- `concurrent_threads ≥ 1`
- `num_chunks ≥ 1`
- `min_chunk_size ≥ 1`, then `max_chunk_size ≥ min_chunk_size` (and ≥ 1)
- `concurrent_verifications ≥ 1`
- `verification_update_interval ≥ 1`
- `verification_buffer_size ≥ 1` (0 is already safe byte-at-a-time; clamp for tidiness)
- `download_timeout_secs ≥ 1`
- `progress_update_interval_ms ≥ 1`

`max_retries`, `retry_delay_secs` and the rate-limit pair are already safe
(0 is meaningful for the first two; 0 rate = unlimited is the documented
contract) and are **not** clamped. Deliberately **not** enforcing the UI's
full display ranges (e.g. threads ≤ 32): that would silently rewrite
legitimate hand-configured values; only the hang/panic floors are enforced.

`normalize_engine_options` is a `pub(crate)` pure fn so the clamp matrix is
unit-testable directly, and `apply_options_maps_every_field_and_snapshot_round_trips`
keeps passing because its values are in-range.

### Fix 3 — `unreachable!()` on the `f` filter-field key

`src/ui/app/events/keys.rs:80` matches `focused_filter_field` against
`0/1/2` and panics on `_`. The field is set from the mouse click/scroll
index (`src/ui/app/mod.rs::handle_filter_click` / `handle_filter_scroll`)
with no range check; `filters::cycle`/`step` already treat any index as a
no-op. Today the render registers exactly three filter rects so it is
unreachable, but a fourth filter field (or a hit-test change) turns the next
`f` press into a panic.

**Design 3:** belt-and-suspenders, both tiny:
- clamp the assigned index to `0..=2` at both mouse assignment sites
  (`focused_filter_field = field_idx.min(2)`),
- replace `unreachable!()` with a defensive default arm that reports the
  field as-is (or "Filter") instead of panicking.

---

## 2. Changes by file

| File | Change |
|---|---|
| `src/engine/enqueue.rs` | Mirror arm builds `queueable` (valid files only); accounting/sends/HUD/rollback use it; docs on `EnqueueOutcome::invalid`, `RegistryMode::Mirror`, `EnqueuePolicy` constructors; rename + re-pin the still-queues characterization test |
| `src/paths.rs` | `sanitize::nearest_existing_ancestor` → `pub(crate)` (no body change) |
| `src/download/mod.rs` | `prepare_download_paths`: replace final-parent-only check with `nearest_existing_ancestor` containment; add `#[cfg(unix)]` containment tests |
| `src/config.rs` | `normalize_engine_options` (pub(crate) pure fn) + call it at the top of `apply_options`; clamp matrix tests + an apply-with-zeros test |
| `src/ui/app/mod.rs` | clamp `focused_filter_field` to `0..=2` at the two mouse sites |
| `src/ui/app/events/keys.rs` | defensive default arm in the `f` match; update the match's contract comment |
| `src/ui/app/events/mod.rs` | W4.6 advance-contract test: out-of-range `focused_filter_field` + `f` press → no panic |
| `AGENTS.md` | update the "mirror flavors report invalid files and still queue them" line (line ~131) to the new behavior |
| `plans/README.md` | index the new plan |

No new dependencies. No schema/registry/event-shape changes. No changes to
the download transport's byte behavior.

## 3. Test matrix

| Test | Pins |
|---|---|
| `enqueue_mirror_dedupes_by_url_and_skips_invalid_files_…` (renamed `…_from_registry_and_queue`) | invalid file reported but NOT sent/accounted/HUD; valid siblings still queued; registry dedup unchanged |
| new: download/mod.rs `#[cfg(unix)]` deep-subdir-symlink containment | filename `Q4_K_M/sub/file.gguf` with `…/model/Q4_K_M` a symlink outside → `FileOutcome::Failed` "path traversal detected"; nested normal path passes |
| new: `normalize_engine_options` clamp matrix | each dangerous zero/out-of-range value → safe floor; in-range values unchanged; `min>max` → `max≥min` |
| new: `apply_options` with zero-heavy `AppOptions` | atomics read the safe floors (and `…snapshot_round_trips` still green) |
| new: events/mod.rs `f`-key with out-of-range field | no panic; status set |
| existing `enqueue_*` characterization suite | unchanged bytes for all valid-file flavors |
| existing `apply_options_maps_every_field_and_snapshot_round_trips` | unchanged (values in-range) |
| existing mouse/filter tests | unchanged |

## 4. Acceptance commands (each phase lands green)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test enqueue                 # enqueue transaction characterization
cargo test config::                # config normalization + apply_options
cargo test --bin rust-hf-downloader download::   # transport unit tests
INSTA_UPDATE=no cargo test         # full suite, snapshots pinned (final gate)
```

(The crate is bin-only: `cargo test <filter>` scopes by name; full
`INSTA_UPDATE=no cargo test` runs all inline + integration targets against
the in-process mock HF server.)

## 5. Non-goals (explicit)

- **Not** enforcing the UI's display ranges in `apply_options` (threads ≤ 32,
  chunks 10–100, …) — only hang/panic floors.
- **Not** changing `prepare_download_paths`' `base_path` contract to carry a
  raw base; Fix 1a closes the author/model-symlink hole at enqueue where the
  raw base is already known.
- **Not** touching `RegistryMode::Disk` abort semantics (CLI already safe).
- **Not** fixing the audit's nice-to-haves (4–6) — separate change.
- **Not** touching `registry` bytes, NDJSON event schema, or TUI snapshot
  golden output.

## 6. Risks / review notes

- Fix 1a is the only behavior change with pinned output; it is a deliberate
  security hardening and its test is updated in the same commit.
- Fix 1b and 3 are behavior-preserving for non-degenerate inputs; guarded by
  new tests.
- Fix 2 changes no pinned bytes (engine atomics are runtime state); the
  `…_snapshot_round_trips` test re-verifies the in-range path is untouched.
