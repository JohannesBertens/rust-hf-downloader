# Deferred defects & deferred behavior changes — the register

Single home of every known defect and deferred behavior change that is
deliberately **not** fixed in the current pass. This register supersedes the
`§8` deferred-defects list of
[plans/readability-maintainability-refactor.md](../plans/readability-maintainability-refactor.md)
(all of its items are folded in below, each with its disposition).

Rules — enforced by `tests/docs_guards.rs`
(`no_bare_plan_section_anchors_in_src`):

- Cite an entry from source comments as `docs/DEFERRED.md#<key>` — never a
  bare `§N.M` plan anchor (bare anchors resolve to nothing findable).
- Every entry names an anchor **Symbol** that must remain greppable in
  `src/`. If your change deletes the symbol, resolve or re-anchor the entry
  in the same PR — the guard fails otherwise.
- **Status** is one of: `open` · `deferred` · `fix in flight (M#)` ·
  `resolved`. Resolved entries stay as history until their anchor symbol is
  itself removed; entries being fixed by the current implementation wave
  ([plans/architecture-simplification-review.md](../plans/architecture-simplification-review.md),
  M0–M6) carry their milestone.
- Picking up an `open`/`deferred` item that changes observable behavior
  needs an owner sign-off first (the Gate-0 discipline).

Seeded by M0 (2026-10). Gate-0 owner sign-offs recorded 2026-10-06:
**B3, B4, B5, B6, R4 approved.**

## registry-atomic-save

- **Symbol:** `save_registry`
- **Status:** resolved (M1, finding R5 — landed 2026-10-06)
- **Finding:** the registry save in `src/registry.rs` was a plain
  `fs::File::create` write — a crash mid-save truncated
  `hf-downloads.toml`, and the load side silently resets to an empty
  registry on parse failure.
- **Remedy (landed):** same-directory temp file (pid-suffixed, so a
  concurrent process never clobbers our temp) + `sync_all()` +
  rename-with-retry via `utils::atomic_rename_with_retry` (the sync twin;
  the retry absorbs Windows `rename` over a file a reader holds without
  share-delete). The temp file never litters (pinned by
  `registry_tests::save_is_atomic_no_temp_litter`); op bytes stayed
  pinned by the `registry_tests` goldens. Durability note: no parent-dir
  fsync after the rename — a power loss can drop the latest rename
  (losing the newest save) but cannot tear the file.
- **Provenance:** readability plan §8 item 5; architecture plan §3.1 R5.

## registry-write-sleep-under-lock

- **Symbol:** `atomic_rename_with_retry`
- **Status:** open
- **Finding:** the registry save runs the SYNC rename-with-retry (up to ~1 s
  of transient-lock backoff sleeps on Windows) while holding `REGISTRY_WRITE`
  inside async callers. Disk writes are rare and the registry TOML is small,
  so the blocking window is brief and bounded — accepted at Gate 2 and
  flagged for future work — but a busy async runtime pays the stall.
- **Remedy:** move the save off the async path (spawn_blocking) or accept
  the documented tradeoff; the utils.rs async twin exists if the callers
  can restructure.
- **Provenance:** architecture-plan run Gate 2 review (GLM-5.3 + Opus
  notes); M1 design tradeoff.

## enqueue-mirror-overwrite-window

- **Symbol:** `seed_registry_mirror`
- **Status:** open
- **Finding:** between `upsert_pending` returning its post-write snapshot
  and the caller replacing the engine mirror (`engine/enqueue.rs` Mirror
  arm), a concurrent `mark_mismatch_mirror` patch can be overwritten in the
  MIRROR ONLY — disk stays correct post-M1, so this is a transient HUD/UI
  display staleness window, not data loss.
- **Remedy:** consolidate mirror mutation through one owner (an M3-follow-on
  applying the same single-writer discipline to the mirror).
- **Provenance:** architecture-plan run Gate 2 review (residual note); M1
  left the window strictly narrower than pre-M1 (which could clobber disk).

## registry-cross-process-lock

- **Symbol:** `load_registry`
- **Status:** deferred (optional follow-on to M1)
- **Finding:** registry writes race across **processes** — a concurrent CLI
  run and TUI session (or two CLI runs) can lose updates. M1's single
  writer is process-global, not cross-process, and the atomic save
  prevents torn files, not lost writes. (The *in-process* variant —
  manager vs verification worker, previously pinned as desired by the
  two-writer tests — **was fixed by M1, landed 2026-10-06** under the R4
  sign-off: every writer's update survives, no lost updates; the
  two-writer tests now assert all-writers-win.)
- **Remedy:** advisory lock file around the write window, if concurrent
  front-ends ever matter enough.
- **Provenance:** architecture plan §8; readability plan §8 (the pinned
  lost-update race).

## complete-downloads-filename-key-collision

- **Symbol:** `CompleteDownloads`
- **Status:** deferred
- **Finding:** `CompleteDownloads` is keyed by bare filename — two models
  downloading a file with the same name overwrite each other's
  completion/HUD entry.
- **Remedy:** key by model/revision/path. Changes registry-adjacent bytes;
  needs its own sign-off. M5 only moves the type next to
  `DownloadMetadata` and documents the key contract.
- **Provenance:** architecture plan §8 / §3.5 U6.

## registry-entry-size-zero

- **Symbol:** `tui_quant`
- **Status:** deferred (sign-off item if picked up)
- **Finding:** `EnqueuePolicy::tui_quant` records resumed GGUFs with
  `RegistryEntrySize::Zero` — the HUD shows 0 B for them. Recording the
  real size changes registry bytes (HUD cosmetics vs registry-bytes
  tradeoff).
- **Remedy:** write the queued file size on the resume path, behind a
  sign-off.
- **Provenance:** architecture plan §8.

## status-kind-tagging

- **Symbol:** `status_line`
- **Status:** deferred by default (optional M5 item)
- **Finding:** engine status lines are free-form strings and the CLI
  `Reporter::status_line` prefix-filters hard-coded message strings —
  rewording an engine message silently changes CLI output. String
  contract: the producers live in `engine`/`download`; the CLI matches
  those exact prefixes.
- **Remedy:** `StatusKind`-style tagging of status lines. Observable
  change; the default is to keep and document the string contract (this
  entry).
- **Provenance:** architecture plan §8 / C3.

## verification-busy-poll

- **Symbol:** `verification_worker`
- **Status:** deferred (until multi-file NDJSON tail snapshots exist)
- **Finding:** the verification worker polls the queue with a 100 ms sleep
  loop instead of being notified; wake latency is visible in NDJSON event
  tails, so replacing the poll changes observable timing.
- **Remedy:** notify/channel wake-up, gated on H6-style multi-file tail
  snapshots pinning current ordering first.
- **Provenance:** readability plan §8 item 12; architecture plan C6.

## short-hash-slice-panic

- **Symbol:** `expected_sha256`
- **Status:** resolved (M2, finding B2 — landed 2026-10-06)
- **Finding:** verification status text sliced `&expected_sha256[..16]` —
  panics on a short/corrupt registry hash.
- **Remedy (landed):** char-safe truncation (`chars().take(16)`) with the
  ellipsis only when actually truncated.
- **Provenance:** readability plan §8 item 3; architecture plan §3.4 B2.

## recursive-tree-error-swallow

- **Symbol:** `fetch_recursive_tree`
- **Status:** resolved (M4, finding B3 — Gate-0 sign-off approved 2026-10-06, landed)
- **Finding:** `src/api/client.rs` `fetch_recursive_tree` silently
  swallows per-directory fetch errors → truncated file trees with no
  signal.
- **Remedy (landed):** propagate subdir errors; e2e asserts a surfaced `network`
  event + exit code instead of silent truncation.
- **Provenance:** readability plan §8 item 1; architecture plan §3.4 B3.

## chunk-task-zombies

- **Symbol:** `spawn_chunk_tasks`
- **Status:** resolved (M4, finding B4 — Gate-0 sign-off approved 2026-10-06, landed)
- **Finding:** failed chunk tasks are detached and never aborted; the
  retry recreates the file beneath zombie workers still writing to the
  old `.incomplete`.
- **Remedy (landed):** `JoinSet` + `abort_all()` on first error, with a
  deterministic abort pin (parked sibling performs no writes after the
  retry recreates the file).
- **Provenance:** readability plan §8 item 2; architecture plan §3.4 B4.

## rate-limiter-zero-rate

- **Symbol:** `RateLimiter`
- **Status:** open
- **Finding:** `src/rate_limiter.rs`: a rate of `0.0` while the limiter
  stays enabled makes `acquire` compute an infinite wait
  (`tokens_needed / 0.0`) and panic in `Duration::from_secs_f64`.
  Construction treats 0 as unlimited/disabled; the runtime rate update
  path does not.
- **Remedy:** treat a runtime rate of 0 as disable (or reject it).
- **Provenance:** readability plan §8 item 4.

## queue-item-filename-only-match

- **Symbol:** `download_queue_items`
- **Status:** open
- **Finding:** the download manager removes the HUD-mirror queue item by
  first **filename** match (the manager loop in `src/engine/workers.rs`, via
  `QueueAccounting::remove_started` in `src/engine/mod.rs` since M3) — two
  queued files sharing a filename across models/revisions remove the wrong
  row.
- **Remedy:** match on model+revision+filename; needs the
  `QueueItemSummary` mirror schema extended.
- **Provenance:** readability plan §8 item 6.

## http-client-per-request-and-silent-header-drop

- **Symbol:** `get_with_optional_token` / `build_client_with_token`
- **Status:** resolved (M4, finding B5 — Gate-0 sign-off approved 2026-10-06, landed)
- **Finding:** `src/http_client.rs` builds a fresh `reqwest::Client` per
  request (TLS handshake per call, no pooling), and an invalid header
  value is silently dropped — the request goes out **unauthenticated**
  instead of failing.
- **Remedy (landed):** one shared client threaded from the Runner bootstrap;
  an invalid header value is dropped WITH an explicit warning
  (`TokenDroppedWarning`) and the run proceeds unauthenticated — never
  the silent downgrade, and (owner revision 2026-10-07 of B5) never a
  run-fatal error either: the CLI emits a `warning` NDJSON event /
  `Warning:` stderr line at bootstrap (all four frontends), the TUI shows
  the warning on the status line, and the download transport sends it
  through the run's status channel.
- **Provenance:** readability plan §8 items 7+11; architecture plan §3.4
  B5.

## options-default-env-token-read

- **Symbol:** `AppOptions::default`
- **Status:** open
- **Finding:** `AppOptions::default()` reads `$HF_TOKEN` — a second env
  source beside the CLI `merge_token` precedence. Unobservable through
  the resolved token (the env axis wins either way; pinned by the
  token-matrix tests in `src/cli/tests.rs`), which is why it is
  tolerated.
- **Remedy:** remove the env read from the default constructor — a
  behavior change requiring sign-off.
- **Provenance:** readability plan §8 item 8.

## options-dialog-transient-state

- **Symbol:** `OptionsDialogState`
- **Status:** resolved (W-wave, W4.7/W5.3 era)
- **Finding:** transient options-dialog UI state (cursor row, live-edit
  flags) used to sit inside `AppOptions`; it moved out to
  `ui/app/options.rs::OptionsDialogState` (W4.7/W5.3 created it in
  `ui/render/options_popup.rs`; M5/U1 moved it to the app layer together
  with the two text-edit buffers), making `AppOptions` purely the
  persisted config schema.
- **Pin:** the TOML golden `appoptions_toml_golden_round_trip` in
  `src/models/options.rs` pins the exact persisted schema.
- **Provenance:** readability plan §8 item 9 (the old `§8.9` anchor).

## update-cmd-progress-divergence

- **Symbol:** `run_update`
- **Status:** resolved (M4, item C5 — landed 2026-10-06)
- **Finding:** `src/cli/update_cmd.rs` hand-rolls its human progress line,
  diverging from the `Reporter` shapes; adopting the Reporter changes
  stderr bytes, which is why it waited for the H4 human-output goldens
  (now landed).
- **Remedy (landed):** unified on Reporter shapes; the H4 goldens make the diff
  reviewable.
- **Provenance:** readability plan §8 item 10; architecture plan C5.

## options-dialog-field-reachability

- **Symbol:** `OPTIONS_FIELDS`
- **Status:** resolved (W-wave, W4.7)
- **Finding:** if the popup cursor bound (`selected_field < 15`) ever
  diverged from the 16 rendered fields, some field would be unreachable.
  W4.7's `OPTIONS_FIELDS` table made the bound derive from the table
  length (16 entries → last index 15), so reachability now follows the
  table automatically.
- **Pin:** `src/ui/app/options_tests.rs` + the options-dialog snapshots.
- **Provenance:** readability plan §8 item 13.
