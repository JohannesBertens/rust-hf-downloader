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
