# Plan: Fix audit findings 1, 2, 3 (ponytail-audit round)

**Status:** proposed (feature branch `feature/fix-audit-1-2-3`)

**Source:** `ponytail-audit` report on this repo (2026-10-17). Three
findings, ordered by severity, all small and independent. This plan fixes
all three in one PR. **Rev 3:** incorporates the second plan-gate review
round. Rev 2 folded the four round-1 reviewers (gemini38 + copilot blocks,
opus + opencode notes); rev 3 folds the fix-confirmation round — opus
raised a new P0 (the reused containment predicate accepts a symlink to an
ancestor of the base) plus three P2s, and opencode one P2 wording fix. All
are folded in below.

| # | Finding | File(s) | Kind |
|---|---------|---------|------|
| 1 | Symlink inside the download folder sends files outside it | `src/engine/enqueue.rs`, `src/download/mod.rs`, `src/paths.rs`, `src/ui/app/downloads.rs` | security / path containment |
| 2 | A hand-edited config with `0` (or reversed) tuning values hangs or panics the engine | `src/config.rs`, `src/download/chunked.rs` | robustness / availability |
| 3 | `unreachable!()` landmine + debug overflow in the `f` filter-field key | `src/ui/app/events/keys.rs` | crash robustness (defense-in-depth) |

---

## 1. Root cause and design

### Fix 1 — path containment

Two compounding sites (the audit's "the same rule applied differently in
two places").

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
on the first invalid file before anything is queued, verified in
`src/registry.rs:265-267`).

**Design 1a:** in the Mirror arm, a file whose path validation fails is
reported via `outcome.invalid` and is **not** queued, not counted in queue
accounting, not sent, and not pushed to the HUD mirror. Concretely, the
enqueue transaction builds `queueable: Vec<&QueuedDownload>` containing
only files that passed validation (Mirror arm) or all files (NoWrites and
Disk arms) — **borrowed references**, so no payload is cloned except the
existing per-send clone. Queue accounting, HUD mirror, sends, and the
failed-send rollback all operate on `queueable` instead of `files`;
`total_bytes` is computed from `queueable`. `sent` then counts only files
actually handed to the channel. Registry bookkeeping is unchanged: the
Mirror arm already excludes invalid files from `entries` (the `upsert_pending`
dedup set), so dedup semantics are untouched.

**UI follow-on (fix 1a):** `finish_enqueue` (`src/ui/app/downloads.rs`)
currently overwrites the per-file invalid error with the generic failure
whenever `sent == 0`. With invalid files no longer queued, an **all-invalid
batch** hits that path and hides the real `PathError`. Change: the
`sent == 0` branch only writes the generic `failure` string when
`outcome.invalid.is_empty()` — otherwise the last invalid-file error stays
visible. Add an all-invalid characterization case.

This is a deliberate behavior change to a byte-pinned transaction: the
`enqueue_mirror_…_still_queues_them` characterization test and the
`EnqueueOutcome::invalid` / `RegistryMode::Mirror` / `EnqueuePolicy` /
`finish_enqueue` doc-comment contracts all flip to "reported, not queued",
and the matching line in `AGENTS.md` is updated.

**1a also hardens the validation it relies on (rev 3, P0):** the strong
check itself — `validate_and_sanitize_path` (`src/paths.rs:416-429`) — uses
the ancestor allowance `base.starts_with(canonical_parent)`, which accepts a
symlink pointing at an *ancestor* of the base (e.g. `…/model/Q4_K_M` → `/`
or `$HOME`): `canonical_parent` canonicalizes to `/`, `base.starts_with("/")`
is true, and the file is written to `/sub/file.gguf` (arbitrary-location
write). The ancestor clause exists only for the not-yet-created-base case,
where the first existing ancestor of the final path **is** `canonical_base`
itself — so a strict `canonical_parent.starts_with(&canonical_base)` covers
that case too. Change: make `validate_and_sanitize_path` require strict
`starts_with(canonical_base)` (drop the ancestor clause), pinned by a new
`sanitize` test "symlink → ancestor of base → `TraversalParentEscape`". All
existing `paths::sanitize` tests still pass (traced: the not-yet-created-
base and existing-base cases both satisfy strict `starts_with`). The
final-path tail can never contain `..` or absolute components — every
component (author, model, filename parts) passes `sanitize_path_component`
before the path is built — so strict `starts_with` on a canonicalized base
plus component sanitization fully closes the tail; no string-prefix check
and no rejoin-with-`..`-rejection step is needed.

**How `canonical_base` is derived (rev 3.1, opus N1):** in
`validate_and_sanitize_path`, `canonical_base` is `canonicalize(nearest_existing_ancestor(base))`
when `base` does not exist (an existing base is `canonicalize(base)`) — the
missing tail is **not** re-joined, so the first existing ancestor of the
final path is the same anchor and strict `starts_with` passes the
not-yet-created-base case. This is pinned by an existing sanitize test
(`validate_accepts_not_yet_created_base_directory`) plus a new explicit
"base absent, parent exists → Ok" assertion if needed.

**1b. `prepare_download_paths` containment is final-parent-only and skips
symlinked final components** (`src/download/mod.rs`).

Two gaps:
- The check canonicalizes only `final_path.parent()` and only when that
  parent already exists. A *deep* symlinked subdir whose child directories
  do not exist yet (e.g. filename `Q4_K_M/sub/file.gguf` where
  `…/model/Q4_K_M` is a symlink elsewhere and `sub` is not created yet)
  escapes and the file lands outside the base. `sanitize::
  validate_and_sanitize_path` already implements the robust rule — walk up
  to the **first existing ancestor**, canonicalize it, and require it to be
  under the canonical base (or an ancestor of the base).
- If `final_path` or the `.incomplete` write target is itself a symlink,
  every ancestor check passes and the write follows the link outside the
  base.

**Design 1b:**
- Make `paths::sanitize::nearest_existing_ancestor` `pub(crate)` and reuse
  it in `prepare_download_paths`. **Pass `final_path` itself** — the helper
  excludes its argument and walks the parents, so passing `final_path`
  checks `final_path.parent()` first (preserving the current direct-parent
  behavior) and then walks up when the parent does not exist yet.
- **Strict containment (rev 3, P0):** require `canonical.starts_with(&canonical_base)`
  only — **drop the `base.starts_with(canonical)` ancestor clause**. In
  `prepare_download_paths` the `base_path` is always created
  (`create_dir_all(base_path)`) before the check, so `canonical_base` is a
  real, existing base; the ancestor clause would then be reachable only by
  an escape (a symlink to `/` or `$HOME` canonicalizes to an ancestor of the
  base and `base.starts_with(ancestor)` is true → arbitrary-location
  write). The legit not-yet-created-base case needs no clause (traced
  above). The check runs **before any `create_dir_all`** for the deep
  subdirs (the existing code already orders it that way —
  `create_dir_all(base_path)` happens first, then the check, then the
  deep-parent mkdirs).
- Reject a symlink at the **final path, the `.incomplete` path, or the
  resume-journal sidecar** (`journal_sidecar_path`) via
  `symlink_metadata(...).file_type().is_symlink()` → same traversal error,
  before any open/mkdir (a symlinked `.incomplete` or journal is never
  treated as resume state). A `NotFound` from `symlink_metadata` means
  "proceed"; any other error fails closed.
- Dangling intermediate symlink (rev 3.1, opus N2): if
  `nearest_existing_ancestor` uses `exists()` (which follows links), a
  dangling `…/model/Q4_K_M` is skipped and the walk passes at `model/`;
  rejection then relies on `create_dir_all` failing closed on the symlink
  (`EEXIST`/`is_dir` false). That fail-closed-by-side-effect path is
  accepted and noted; the deep-subdir test adds a dangling-target variant.
- No behavior change for non-symlinked paths (first existing ancestor is
  under the base), so no pinned output changes.

**Residual (recorded, not fixed here):** for the two flows that skip
enqueue-time validation — `tui_resume` (re-queues from registry
`local_path`s recorded at first enqueue) and `hf-cache sync` (downloads into
a tool-created staging dir) — a symlink planted at `base/<author>` (or
`base`) *after* the first download would still be baked into the
pre-built `base/author/model` `canonical_base` and escape. Two further
residuals are recorded in the same `docs/DEFERRED.md` entry (rev 3): (a) a
check-then-open TOCTOU — a symlink swapped in between `symlink_metadata`
and the open still escapes (closing it needs `O_NOFOLLOW`, a dependency
change the plan avoids); (b) `create_dir_all(base_path)` can itself create
directories outside the base via an author symlink before the residual
escape happens. Closing the author/model residual needs passing the raw
base into `DownloadParams` — an explicit non-goal, see §5.

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
| `verification_update_interval = 0` | `iteration % 0` **panics the blocking hash task** (surfaced as a verification error via `JoinError`, not a process abort) |
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
- `min_chunk_size ≥ 1`, then `max_chunk_size ≥ max(min_chunk_size, 1)`
- `concurrent_verifications ≥ 1`
- `verification_update_interval ≥ 1`
- `download_timeout_secs ≥ 1`
- `progress_update_interval_ms ≥ 1`

`verification_buffer_size` is **not** clamped: `stream_file_digest`
(`src/utils.rs:34-36`) explicitly supports `buffer_size = 0` as a
one-byte-at-a-time read mode, so zero is safe and meaningful — clamping it
would conflict with preserving legitimate values (test: zero stays zero).
The direct `buffer_size = 0` digest behavior is already pinned by the
existing `utils::tests::stream_digest_zero_inner_buffer_is_still_exact`
(rev 3 note — no new test needed there).
`max_retries`, `retry_delay_secs` and the rate-limit pair are already safe
(0 is meaningful for the first two; 0 rate = unlimited is the documented
contract) and are **not** clamped. Deliberately **not** enforcing the UI's
full display ranges (e.g. threads ≤ 32): that would silently rewrite
legitimate hand-configured values; only the hang/panic floors are enforced.

**Coverage note (audit finding P1-3, resolved by grep):** the only readers
of these `AppOptions` fields outside `config.rs` are the options screen
(`modify_option`, which already clamps before writing `self.options`) and
the display-only table (`ui/app/options.rs`). Every engine consumer
(semaphores, timeouts, chunk math) reads the `DOWNLOAD_CONFIG` /
`VERIFICATION_CONFIG` atomics that `apply_options` fills, and both frontends
call `apply_options` before any download/verification runs (CLI
`run::load_run_config`; TUI `App::run` → `sync_options_to_config`, which
delegates to `apply_options` — `App::new` itself does not call it, rev 3
wording fix). So normalizing inside `apply_options` reaches every reachable
engine path. The zero-heavy test asserts the atomics hold the floors — the
exact values the semaphores/timeouts are built from at use time.

`normalize_engine_options` is a `pub(crate)` pure fn so the clamp matrix is
unit-testable directly, and `apply_options_maps_every_field_and_snapshot_round_trips`
keeps passing because its values are in-range.

### Fix 3 — `unreachable!()` landmine + debug overflow on the `f` key

**Corrected root cause (plan-gate round):** the audit's "one keypress can
crash the TUI" framing was wrong. `src/ui/app/events/keys.rs:75` evaluates
`(self.focused_filter_field + 1) % 3` *before* the `0/1/2` match, so for
any ordinary index (e.g. `3`) the matched value is already in `0..=2` and
`unreachable!()` cannot fire — a future 4th filter field would cause a
wrong-cycle, not a panic. Two real issues remain:
1. `(x + 1)` overflows `usize` in **debug builds** when
   `focused_filter_field == usize::MAX` — a hostile/glitched index panics
   before the `% 3`.
2. `unreachable!()` is a landmine: if the `% N` modulus is ever changed
   without updating the match, it panics.

**Design 3:** belt-and-suspenders, both tiny, in `keys.rs` only (no mouse
clamps — a `field_idx.min(2)` would hardcode the field count and silently
map a future 4th field onto field 2, desyncing focus from the cycled
filter):
- `self.focused_filter_field = (self.focused_filter_field % 3 + 1) % 3`
  — normalize **before** incrementing: `% 3` first bounds the value to
  `0..=2`, so `+ 1` can never overflow for any `usize`.
- replace `_ => unreachable!()` with a defensive default arm
  (`_ => "Filter"`) so the match never panics even if the modulus changes.

## 2. Changes by file

| File | Change |
|---|---|
| `src/engine/enqueue.rs` | Mirror arm builds `queueable: Vec<&QueuedDownload>` (valid files only); accounting/sends/HUD/rollback use it; docs on `EnqueueOutcome::invalid`, `RegistryMode::Mirror`, `EnqueuePolicy` constructors; rename + re-pin the still-queues characterization test |
| `src/ui/app/downloads.rs` | `finish_enqueue`: `sent == 0` branch only writes the generic failure when `invalid.is_empty()`; update the stale "still queued and sent" comments (lines ~16-17 and ~230-232); all-invalid characterization case |
| `src/paths.rs` | `sanitize::nearest_existing_ancestor` → `pub(crate)` (no body change); `validate_and_sanitize_path` drops the ancestor clause and requires strict `starts_with(canonical_base)`; new `#[cfg(unix)]` symlink-to-ancestor-of-base test (`TraversalParentEscape`) |
| `src/download/mod.rs` | `prepare_download_paths`: reuse `nearest_existing_ancestor(final_path)` with strict `starts_with(canonical_base)` containment; reject symlinked `final_path`/`.incomplete`/journal sidecar via `symlink_metadata`; add `#[cfg(unix)]` containment tests |
| `docs/DEFERRED.md` | new entry `download-author-model-symlink-residual` (anchor `prepare_download_paths`, status deferred) — the planted-after-the-fact author/model symlink residual for `tui_resume`/`hf-cache sync`, **plus the check-then-open TOCTOU and the `create_dir_all` blast radius** (rev 3) |
| `src/config.rs` | `normalize_engine_options` (pub(crate) pure fn) + call it at the top of `apply_options`; clamp matrix tests + zero-heavy apply test |
| `src/ui/app/events/keys.rs` | normalize-before-increment `(x % 3 + 1) % 3`; `_ => "Filter"` default arm; update the contract comment |
| `src/ui/app/events/mod.rs` | advance-contract test: `focused_filter_field = usize::MAX` (and `3`) + `f` press → no panic, field in `0..=2`, status set |
| `AGENTS.md` | update the "mirror flavors report invalid files and still queue them" line (line ~131) to the new behavior |
| `plans/README.md` | index the new plan |

No new dependencies (tests use the crate's existing temp-dir convention,
not `tempfile`). No schema/registry/event-shape changes. No changes to the
download transport's byte behavior.

## 3. Test matrix

| Test | Pins |
|---|---|
| `enqueue_mirror_dedupes_by_url_and_skips_invalid_files_…` (renamed `…_from_registry_and_queue`) | invalid file reported but NOT sent/accounted/HUD; valid siblings still queued; registry dedup unchanged |
| new: all-invalid `finish_enqueue` characterization | `sent == 0` + non-empty `invalid` → the `PathError` stays visible, not the generic failure |
| new: download/mod.rs `#[cfg(unix)]` deep-subdir symlink | filename `Q4_K_M/sub/file.gguf` with `…/model/Q4_K_M` a symlink outside and `sub` absent → `FileOutcome::Failed` "path traversal detected" |
| new: download/mod.rs `#[cfg(unix)]` existing symlinked parent | filename `dir/file.gguf` with `…/model/dir` an existing symlink outside → traversal error (preserved current behavior) |
| new: download/mod.rs `#[cfg(unix)]` symlink → ancestor of base | filename `Q4_K_M/sub/file.gguf` with `…/model/Q4_K_M` a symlink to the **temp root** (never `/` — rev 3.1, opus N4, so a predicate regression cannot write outside the test sandbox) → traversal error (strict predicate; the old ancestor clause would pass it) |
| new: download/mod.rs `#[cfg(unix)]` symlinked final path | `final_path` itself a symlink → traversal error, never treated as complete |
| new: download/mod.rs `#[cfg(unix)]` symlinked `.incomplete` | the `.incomplete` write target itself a symlink → traversal error (its own case, rev 3.1, opus N3 — the real write target, not covered by the final-path case) |
| new: download/mod.rs `#[cfg(unix)]` symlinked journal sidecar | the resume-journal sidecar (`journal_sidecar_path`) itself a symlink → traversal error (its own test, rev 3.1 — opus P2-b wants the sidecar covered independently) |
| new: download/mod.rs `#[cfg(unix)]` dangling intermediate symlink | `…/model/Q4_K_M` a dangling symlink → download fails closed (via `create_dir_all`/`is_dir`), no outside write (rev 3.1, opus N2) |
| new: download/mod.rs `#[cfg(unix)]` normal nested path | `Q4_K_M/sub/file.gguf` with real dirs → `Ok`, no false rejection |
| new: paths/sanitize `#[cfg(unix)]` symlink → ancestor of base | `validate_and_sanitize_path` rejects a symlinked subdir pointing at the temp root (or `$HOME`) with `TraversalParentEscape` (regression for the strict predicate; rev 3.1, opus N4: temp root, not `/`) |
| existing paths/sanitize fresh-base test | `validate_accepts_not_yet_created_base_directory` already pins the not-yet-created-base Ok case under strict `starts_with` (rev 3.1, opus N1) |
| new: `normalize_engine_options` clamp matrix | each dangerous zero/out-of-range value → safe floor; in-range values unchanged; `min>max` → `max≥min`; `verification_buffer_size = 0` stays 0 |
| new: `apply_options` with zero-heavy `AppOptions` | atomics read the safe floors (and `…snapshot_round_trips` still green) |
| new: events/mod.rs `f`-key with `usize::MAX` and `3` | no panic (debug overflow pre-fix catches `usize::MAX`); field wraps into `0..=2`; status set |
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
  raw base is already known, and the `tui_resume`/`hf-cache sync` residual is
  recorded in `docs/DEFERRED.md` instead (see Fix 1b).
- **Not** touching `RegistryMode::Disk` abort semantics (CLI already safe).
- **Not** fixing the audit's nice-to-haves (4–6) — separate change.
- **Not** touching `registry` bytes, NDJSON event schema, or TUI snapshot
  golden output.
- The release version bump + CHANGELOG/README changelog-table row are **not**
  part of this PR — this repo bumps versions at release time (a separate
  release commit, per `changelog/README.md` and the README table); this
  branch ships the code + plan + tests only.

## 6. Risks / review notes

- Fix 1a is the only behavior change with pinned output; it is a deliberate
  security hardening and its test + docs are updated in the same commit.
  `finish_enqueue`'s error-preservation change is a direct consequence and
  is pinned by the all-invalid case.
- Fix 1b and 3 are behavior-preserving for non-degenerate inputs; guarded by
  new tests. Fix 3's implementation (normalize-before-increment, no mouse
  clamps, defensive default arm) addresses both plan-gate blocks.
- Fix 1's strict-predicate change (rev 3, P0) hardens both
  `validate_and_sanitize_path` and `prepare_download_paths`; it preserves
the not-yet-created-base and existing-base cases (traced in §1), pinned by
the new symlink-to-ancestor tests. The remaining symlink residuals
(check-then-open TOCTOU, `create_dir_all` blast radius, author/model for
the two no-write flows) are recorded in `docs/DEFERRED.md`.
- Fix 2 changes no pinned bytes (engine atomics are runtime state); the
  `…_snapshot_round_trips` test re-verifies the in-range path is untouched.
