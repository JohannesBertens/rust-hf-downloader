# Testing Guide for Rust HF Downloader

How to actually run this crate's tests. The suite is **bin-only**: unit
tests live inline (`#[cfg(test)]`) in `src/**` modules of the binary crate,
integration tests live in `tests/*.rs` (one cargo target per file), and the
snapshot goldens (`insta`) pin the TUI render/style output and the CLI
human-output bytes. There are **no** benchmark, doc-test, or coverage
targets — and no PR/push CI: the only CI is the tag-triggered release
workflow, which runs `cargo test --locked` on the three native runners
(Linux, macOS, Windows). Local gates are therefore the primary guard.

`tests/docs_guards.rs::testing_md_targets_exist` checks that every target
named here exists — keep this file truthful.

## Quick Reference (real targets only)

```bash
# Format / lint
cargo fmt --check            # or: cargo fmt
cargo clippy --all-targets -- -D warnings

# Everything (inline unit tests + all integration targets)
cargo test

# One integration target
cargo test --test cli_download

# Filter by name, across all targets (substring match)
cargo test enqueue           # e.g. the 8 EnqueuePolicy characterization tests
cargo test --test cli_exit_codes -- auth   # filter inside one target

# Snapshot gate: run with snapshot writes DISABLED (see below)
INSTA_UPDATE=no cargo test

# Show println output
cargo test -- --nocapture
```

There is no library target (no `src/lib.rs`) — do not look for a lib-only
test invocation; `cargo test <filter>` already scopes by name.

## Integration targets (`tests/`)

Each `tests/<name>.rs` file is one cargo target; run with
`cargo test --test <name>`.

| Target | What it pins |
|---|---|
| `tests/cli_download.rs` | `download` subcommand end-to-end: real binary vs in-process mock HuggingFace server (hyper, Range-aware), full isolation via `RUST_HF_DOWNLOADER_CONFIG_DIR`/`_DATA_DIR` |
| `tests/cli_exit_codes.rs` | Exit-code matrix (H3) + non-TTY human-output goldens (H4), incl. the update/checksum codes |
| `tests/download_failures.rs` | Wire-level download failure injection: exit codes, on-disk state, final registry state through the W5.1a/b phases |
| `tests/hf_cache_sync.rs` | `hf-cache sync` / `hf-cache path` end-to-end: staging→publish, layout, exit codes |
| `tests/update_e2e.rs` | Self-update flow against a fake release dir served over local HTTP (temp-copy binary swaps itself) |
| `tests/docs_guards.rs` | Documentation truth guards: AGENTS.md lock-hierarchy completeness, §-anchor/DEFERRED hygiene, TESTING.md target existence |

`tests/common/mod.rs` is the shared harness (mock server, env isolation) —
not a target itself.

## Unit-test inventory (inline `#[cfg(test)]` in `src/**`)

Skeleton: module → what it pins → how to run. The CLI rows are
per-subject modules (M6/T1: the former `cli/tests.rs` grab-bag was split
so each subject is discoverable and independently runnable).

| Module | What it pins | Run |
|---|---|---|
| `src/config.rs` | config load/save paths; ENGINE_OPTIONS declarative table (M6/C4) — per-field apply + snapshot round-trip pin | `cargo test config` |
| `src/paths.rs` | app path-resolution precedence + sanitize security (hub-cache tests moved to cache_layout, M6/C7) | `cargo test paths` |
| `src/registry.rs` + `src/registry/registry_tests.rs` | byte-exact TOML goldens of every typed registry op, concurrency/failure contracts | `cargo test registry` |
| `src/engine/enqueue.rs` | 8 EnqueuePolicy characterization tests (the single enqueue transaction) | `cargo test enqueue` |
| `src/engine/workers.rs` | manager drain/join contract | `cargo test workers` |
| `src/engine/bootstrap.rs` | bootstrap sequence | `cargo test bootstrap` |
| `src/engine/mod.rs` | `verification_idle` semantics | `cargo test verification_idle` |
| `src/fmt.rs` | frozen-oracle tables for every formatter variant | `cargo test fmt` |
| `src/patterns.rs` | Python-fnmatch parity + `--for vllm` preset tables | `cargo test patterns` |
| `src/api/*` | model filter/sort oracles, quant classification, tree building | `cargo test api` |
| `src/models/*` | AppOptions TOML golden (config schema), engine/cache type invariants | `cargo test models` |
| `src/cli/run.rs` | Runner helpers (tally agreement, verify-outcome counters, run-tail order), token precedence matrix via the production resolvers, model-id gate wording + exit code (M6/C1) | `cargo test run` |
| `src/cli/mod.rs` | dispatch + exit-code constants | `cargo test cli::` |
| `src/cli/args_tests.rs` | args helpers (`valid_model_id`, `merge_token`, rate-limit overrides incl. the full cross-product, `parse_revision`, `ModelDto`) + clap parsing of `download`/`search` + the 27-cell token-precedence matrix | `cargo test args_tests` |
| `src/cli/resolve_tests.rs` | `resolve_files` over every Selector flavor, ambiguity/miss error shapes, `parse_selector` conflicts, `FileSpec` sibling mapping (shared with hf-cache selection) | `cargo test resolve_tests` |
| `src/cli/report_tests.rs` | progress-line formatters, plain-mode heartbeat throttle + skip-on-missed-snapshot through production `poll_once` | `cargo test report_tests` |
| `src/cli/events_tests.rs` | NDJSON event insta goldens (byte-stable wire schema, `src/cli/snapshots/`) + literal wire-bytes tables for the error/file_complete variants | `cargo test events_tests` |
| `src/cli/hf_cache_tests.rs` | §2.2 selection precedence, ref/symlink/absolute-path policy helpers, hf-cache clap surface | `cargo test hf_cache_tests` |
| `src/cli/help_snapshot_tests.rs` | 7 long-help goldens (flag order/grouping/text of every command) | `cargo test help_snapshot` |
| `src/cli/cli_surface_tests.rs` | root surface: version truth, no-subcommand-means-TUI, clap debug_assert | `cargo test cli_surface_tests` |
| `src/cli/testutil.rs` | shared fixtures only (no tests): `file_spec`/`metadata_with`, the `snap!` snapshot macro, `SharedStderr`, `VarGuard` | — |
| `src/cli/hf_cache/sync.rs` | publish gate (M6/C2): every gate arm — Ok / no-verify warning order / mismatch deletes staged bytes / verification error / missing result / failed-download skip / verification-off — with synthetic outcomes, no engine | `cargo test publish_gate` |
| `src/rate_limiter.rs` | token-bucket refill math | `cargo test rate` |
| `src/verification.rs` | verify outcomes, result counters | `cargo test verification` |
| `src/update.rs` | version compare, manifest/asset selection | `cargo test update` |
| `src/utils.rs` | digest streaming + atomic rename with retry | `cargo test utils` |
| `src/cache_layout.rs` | hub-cache layout math, blob/refs naming, sync lock; hub-cache dir resolution (`hf_hub_cache` env precedence) + CACHEDIR.TAG (M6/C7) | `cargo test cache_layout` |
| `src/ui/app/*` | filter cycle/step rules, keyboard dispatch/advance contract, download flows, mouse hit-areas | `cargo test app` |
| `src/ui/render/*_tests.rs` | snapshot / hud / style-size suites (`src/ui/render/snapshots/`) | `cargo test render` |
| `src/ui/tree.rs` | flatten/toggle navigation model | `cargo test tree` |

## Snapshot tests (insta) — the local gate

The TUI render/style snapshots (`src/ui/render/snapshots/`), the CLI
goldens (`src/cli/snapshots/`), and the human-output goldens under
`tests/` are pinned with `insta`. Plain `cargo test` FAILS on drifted or
missing snapshots but never tells you which `.snap.new` files were left
behind or which `.snap` files are now dead — those are LOCAL gate steps
(documented here rather than automated, per the release-only CI reality):

```bash
# 1. Run with writes DISABLED — a red test means a snapshot drifted (or
#    a new one is needed); nothing is modified on disk.
INSTA_UPDATE=no cargo test

# 2. After accepting snapshots (cargo insta accept, or renaming each
#    .snap.new to .snap by hand), scan for strays — a leftover .snap.new
#    means a snapshot was generated but never accepted:
find src tests -name '*.snap.new' -print   # must print nothing

# 3. Unreferenced-snapshot check — every committed .snap must belong to
#    a live test (a deleted test's snapshot is dead weight and, worse, a
#    false sense of coverage). With cargo-insta installed (handles
#    dynamically-built names correctly):
cargo insta test --unreferenced reject
# Without it, grep by leaf name (everything up to the last `__` is the
# module prefix). KNOWN LIMITATION: snapshots named through format!()
# labels (the size_matrix_*_* and hud_threshold_* families) will not
# literally appear in the sources — treat only UNfamiliar names as dead:
ls src/ui/render/snapshots | sed 's/\.snap$//; s/.*__//' \
  | while read -r name; do grep -rq -- "$name" src tests || echo "UNREFERENCED: $name"; done
```

Never edit a `.snap` by hand to make a test pass — regenerate it and
review the diff.

## Tests that touch env vars or global atomics MUST take `ENV_MUTEX`

Cargo runs unit tests as parallel threads of one process; the ambient
environment and the crate-wide atomics (`DOWNLOAD_CONFIG`,
`VERIFICATION_CONFIG`, `RATE_LIMITER`) are shared global state. Every test
that reads/sets env vars (`HF_TOKEN`, `HF_ENDPOINT`,
`RUST_HF_DOWNLOADER_*`, …) or mutates those atomics serializes on
`paths::ENV_MUTEX` (`src/paths.rs`), holding it for the whole test:

```rust
// Take the mutex for the whole test; recover from a poisoned lock rather
// than cascading the failure into every later env/atomics test — a panic
// in one such test must not take the rest down with it.
let _env = crate::paths::ENV_MUTEX
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
```

Conventions that build on this:

- `VarGuard` (`src/cli/testutil.rs`) restores one env var on drop; the
  engine submodule tests share `EnvGuard`
  (`src/engine/mod.rs::test_support`) which redirects
  `RUST_HF_DOWNLOADER_DATA_DIR` + `HF_ENDPOINT` and restores both on
  drop.
- Tests holding `ENV_MUTEX` across an `await` mark it with
  `#[allow(clippy::await_holding_lock)]` — the (std) mutex intentionally
  serializes env-mutating tests while other tokio workers keep making
  progress.

## Local gates (no PR CI — stricter than the release workflow)

Run per commit — the tag-triggered release workflow will run
`cargo test --locked` on all three native runners, so anything
platform-sensitive must hold locally first:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings   # --all-targets: covers test code too
cargo test
INSTA_UPDATE=no cargo test        # + the stray-scan and unreferenced checks above
```

## Troubleshooting

- A test that fails only sometimes in a full run but passes alone is
  usually an env/atomics collision — check the test takes `ENV_MUTEX`
  (see above).
- The integration tests spawn local HTTP mock servers and set
  `HF_ENDPOINT`; nothing here needs the network.
- If the first build takes minutes: that is the dependency tree, not a
  hang. Subsequent runs are incremental.
