# Release Notes - Version 2.13.1

**Release Date**: 2026-10-05

A cleanup release: user-facing fixes from a review pass over the
v2.12.0/v2.13.0 progress features, plus an internal restructuring of the
CLI module. No new features.

## Bug fixes

- **`--help` default rendered twice**: clap already appends
  `[default: auto]` for `--progress` (after the possible-values list);
  the flag's doc comment repeated it, so `download --help` and
  `hf-cache sync --help` showed it twice. The doc comment no longer
  duplicates it.
- **Heartbeat lock artifact**: while SHA256 verification is draining,
  `--progress plain` prints a `verifying: N in flight, M verified`
  heartbeat. If the monitor's lock-free snapshot of the verification
  progress list was momentarily unavailable, the heartbeat printed
  `0 in flight` — presenting a lock miss as a fact. It now skips that
  tick entirely.
- **Doc examples didn't match real output**: README and changelog showed
  the aggregate progress line as `12.6/29.1 GB │ 88 MB/s`; the tool
  always prints two-decimal sizes and one-decimal speed
  (`12.63 GB/29.06 GB │ 88.0 MB/s`). Examples corrected everywhere.

## Internal

- `src/cli.rs` (4.4k lines) split into a `src/cli/` module directory —
  `mod` (Cli/Command/run), `args`, `resolve`, `events`, `report`,
  `download_cmd`, `search_cmd`, `hf_cache_cmd`, `update_cmd`, `tests` —
  one file per section of the original monolith. A pure move: public
  paths (`cli::Cli`, `cli::run`, `cli::EXIT_*`) unchanged, help and
  `--version` output byte-identical, the two engine bootstraps moved
  (not copied), lock-ordering code untouched, and all insta snapshots
  keep their names (now under `src/cli/snapshots/`).
- `Reporter` gained an injectable stderr sink so the plain-mode
  heartbeat can be unit-tested (line format, the shared ~10 s throttle
  window, and the skip-on-missed-snapshot gate). Production output is
  unchanged.

## Tests

- 261 → 266 tests. New: heartbeat emission + throttle + skip-gate unit
  tests; e2e pins for `--help` default rendering (exactly one
  `[default: auto]`), single-file `--progress plain` (file line, never
  the aggregate), piped `auto`-mode silence, and
  `hf-cache sync --progress plain`.
