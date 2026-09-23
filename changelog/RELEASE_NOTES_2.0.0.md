# Release Notes - Version 2.0.0

**Release Date**: 2026-09-23

## Breaking Change: Headless/CLI Mode Removed

### Problem
The application carried two parallel frontends: a ratatui TUI and a `--headless` CLI mode
(`search`, `download`, `list`, `resume` subcommands with `--json`/`--dry-run` flags).
The CLI path required ~1,730 lines of dedicated code (`src/cli.rs`, `src/headless.rs`,
a headless branch in `src/main.rs`, the `clap` dependency, CLI examples and docs),
duplicated the download-manager bootstrap, and had drifted (stale version string,
ignored `--sort` flag, hand-rolled JSON output).

### Solution
The binary is now a pure TUI application. All CLI-only code was removed in one pass;
the shared core (api, download, verification, registry, rate limiting, config) is
untouched and fully covered by the test suite.

### Changes
- **Removed**: `src/cli.rs` (75 lines), `src/headless.rs` (1,415 lines incl. its tests),
  the headless branch of `src/main.rs` (~228 lines; main is now a ~28-line TUI entry point)
- **Removed**: `clap` dependency; `serde_json` direct dependency (remains transitively via reqwest)
- **Removed**: `ModelMetadata.gated` field (its only consumer was headless gated-model checking;
  serde ignores the API's `gated` key since the struct has no `deny_unknown_fields`)
- **Removed**: `examples/headless/` (CI workflow example, download/search/timing scripts)
- **Docs**: README "CLI Mode" and "CLI Reference" sections deleted; descriptions reworded to TUI-only
- **Migration**: scripting/automation users should pin `v1.4.0` or drive the TUI via a PTY harness

### Also in this release
- Snapshot test suite (15 insta snapshots) and 46 new unit tests; suite total 51 after removal
- Fixed options-popup help/field collision on short terminals (adaptive compact layout)
- Fixed GGUF files-pane truncation of long shard filenames (middle ellipsis, tail preserved)

### Files Modified
`src/main.rs`, `src/models.rs`, `src/api.rs` (test fixtures), `Cargo.toml`, `Cargo.lock`,
`README.md`, `plans/README.md`

**Breaking**: yes — major version bump per Semantic Versioning.
