# Release Notes - Version 2.13.2

**Release Date**: 2026-10-05

An internal-only release: a behavior-preserving readability and
maintainability restructuring of the entire crate. No new features, no
user-facing behavior changes — binaries are functionally identical to
2.13.1.

## Internal

Executes `plans/readability-maintainability-refactor.md` in full
(PR #43, 75 commits, +22.4k/−10.5k across 141 files):

- **One shared engine facade** (`src/engine/{mod,enqueue,workers,bootstrap}`):
  every queue handoff — all four TUI download flows and both CLI
  frontends — now goes through the single `EngineState::enqueue`
  transaction with a **sealed** `EnqueuePolicy` (5 named constructors;
  per-frontend divergences are explicit named knobs: `RegistryMode`,
  `SendDiscipline`, `InvalidPolicy`). One `bootstrap()` mirror-seed
  home replaces the duplicated startup sequences.
- **Typed registry ops**: every registry mutation routes through
  `register_pending` / `upsert_pending` / `upsert_metadata` /
  `mark_complete` / `mark_failed` / `mark_mismatch`; the deferred
  lost-update race is now pinned by a test.
- **God-file splits**: `models/`, `api/`, `ui/render/` (2873 → 8 modules
  ≤ ~460 lines), `download/`, `events/`, `cli/hf_cache/`, and a shared
  `cli/run.rs` cross-command runner (`download_cmd.rs` 580 → 155 lines).
- **Foundations**: `fmt.rs` (all human formatting in one place,
  differentially tested against 433 oracle comparisons), `paths::sanitize`,
  a `QueuedDownload` struct, `ErrorCode`/`FileStatus` enums, streaming
  digests, unified multipart parsing, typed `PathError`, `Completion` enum.
- **UI**: pure render pass (hit-rects returned per frame, registration
  order pinned), `FilterState` single home for filter/sort values,
  `MouseState`/`RenderCache`, transient dialog state extracted from the
  `AppOptions` config schema.

## Behavior-preservation evidence

| Contract | Enforcement |
|---|---|
| TUI rendering | all baseline snapshot bodies byte-identical (14 moved verbatim, 0 edited) |
| `--help` output | 7 long-help snapshots taken before the clap flatten — bytes never changed |
| NDJSON wire format | 13-code error table + `FileStatus` + available-payload goldens, byte-exact |
| Exit codes | e2e matrix incl. mixed outcomes (auth-beats-failure = 2; 1-ok+1-failed = 1) |
| Config / registry TOML | registry op byte-goldens through the typed-op migration; `AppOptions` TOML golden round-trip |
| Legacy semantics | characterization suites for every N→1 consolidation (enqueue policies ×8, confirm flows ×8, filter/sort oracle + matrix, multipart corpus ×47, advance tables, token-precedence matrix) |

## Tests

266 → 431 tests; every commit individually gated (full suite +
clippy-zero + fmt-clean). New families: style-signature, size-matrix,
HUD-threshold, enqueue-policy, exit-code matrix, registry byte-goldens.

## Windows CI fixes

The new test families were only ever exercised on Linux; the v2.13.2
tag build caught three Windows-only issues (28 failures, run
37419080867), all in test code:

- The registry TOML byte-goldens expected `"C:\\Users\\…"` (escaped
  basic string), but the `toml` serializer emits a literal string
  (`'C:\Users\…'`, raw separators) when a value contains backslashes.
  The `esc()` helper now produces exactly the token `toml` emits;
  Linux bytes are unchanged.
- The token-matrix temp config dirs embedded `{env:?}` → `Some("env")`;
  the Debug quotes are invalid Windows filename characters
  (CreateFile error 123). Stripped.
- One golden assert panicked while holding the crate-wide `ENV_MUTEX`,
  poisoning it — every later `.lock().unwrap()` on it (19 tests across
  engine/, ui/app) then failed with `PoisonError`. All test-side
  `ENV_MUTEX` locks now recover from poisoning, matching the pattern
  the registry and options tests already used.
- The engine/ui test `EnvGuard`s isolated the registry by redirecting
  `HOME` — but `dirs::home_dir()` reads `USERPROFILE`, not `HOME`, on
  Windows, so every engine test leaked its registry writes to the
  runner's real home and read each other's leftovers (empty-registry
  and exact-count asserts failed nondeterministically). The guards now
  redirect `RUST_HF_DOWNLOADER_DATA_DIR`, which moves the registry path
  on every platform.
- The `register_pending` golden expected its new entry's `local_path`
  built with `Path::join(rel)` (keeps `/` inside the rel on Windows),
  but production builds it one component per join (native separators
  throughout). The expected value is now built the same way
  (`local_parts`); POSIX bytes unchanged.
