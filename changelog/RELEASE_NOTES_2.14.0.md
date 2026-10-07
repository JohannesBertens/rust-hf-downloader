# Release Notes - Version 2.14.0

**Release Date**: 2026-10-07

The architecture-simplification plan, executed end-to-end: a
four-wave implementation of all six milestones (M0–M6) from
`plans/architecture-simplification-review.md`, plus one post-gate
owner revision. 431 → 464 tests. Behavior changes are deliberate and
listed first.

## User-visible changes

- **Invalid `HF_TOKEN` now warns instead of failing** (owner revision
  of the in-branch M4 hard error, which never shipped): a token with
  bytes that cannot appear in an `Authorization` header is dropped with
  an explicit warning while the run **proceeds unauthenticated**
  (exit 0 — unchanged from 2.13.2). Gated repos then fail later with a
  genuine 401 (`auth_required`, exit 2), with the reason already on
  screen. New additive NDJSON event `warning`
  (`{"type":"warning","message":"…"}`, emitted at most once, before any
  other event or request): `download --json`/`hf-cache sync --json` put
  it on stdout; `search --json` and `hf-cache path` print it on
  **stderr** so their stdout stays a single JSON document / clean
  capture. Human mode: `Warning: …` on stderr. TUI: status line (a
  repaired token replaces it with `Token updated`). The `message` text
  is not a stable contract; strict parsers should ignore unknown
  `type` values.
- **Subdirectory tree-fetch failures now surface** (B3): a failing
  subdirectory during file-tree resolution is a visible `network` error
  with a non-zero exit (was: silently truncated file tree).
- **Dead multipart-SHA256 fetch removed** (B6): the transient
  "fetching SHA256s…" status line is gone; every queued part already
  carried its own hash.
- **Chunk zombies no longer accumulate** (B4): when a chunked download
  fails mid-flight and retries, the previous attempt's in-flight chunk
  tasks are aborted instead of running detached beneath the new file.

## Correctness fixes

- **Multi-part verification verifies the selected shard** (B1): manual
  verification of a multi-part quantization no longer verifies
  `files[0]` regardless of selection.
- **Short-hash panic guard** (B2): verification truncation is
  char-safe (`chars().take(16)`, conditional ellipsis) — an
  `expected_sha256` shorter than 16 chars no longer panics.
- **Registry single-writer + atomic save** (M1/R4): all registry
  mutations serialize through `with_registry` under a terminal-leaf
  lock; saves are same-dir temp + fsync + rename-with-retry. The
  in-process lost-update race is closed (all writers win); the
  cross-process window remains the only known, documented data-loss
  gap.
- **Shared HTTP client** (M4): one `reqwest::Client` per run threaded
  from the bootstrap (no TLS handshake per request); the update
  progress line uses the shared reporter.

## Internal

- **Engine decoupled into a DAG** (M3): `verification.rs` and
  `download/` no longer import `crate::engine`; `EngineState`'s 17
  fields are regrouped into owner-scoped bundles
  (`QueueAccounting`, `EventBus`, `VerificationHub`), documented in a
  source-derived, fail-closed lock-hierarchy guard.
- **Options inversion** (M5/U1): the 16-field options dialog state
  lives in `ui/app/options.rs`; rendering is a pure consumer.
  `EnqueuePolicy` collapsed (`InvalidPolicy` deleted, no-write variants
  merged to `NoWrites`, equivalence-pinned).
- **Docs truth floor** (M0): `docs/DEFERRED.md` register (18 entries:
  8 resolved / 10 open) + 5 self-enforcing guard tests in
  `tests/docs_guards.rs`; TESTING/README/CONTRIBUTING/TROUBLESHOOTING
  rewritten to match reality.
- **Test organization** (M6): `cli/tests.rs` split into 7 per-subject
  files + shared testutil; `ENGINE_OPTIONS` declarative config table.

## Note for `--json` consumers

The `warning` event is the only wire addition. Consumers with closed
`Event` enums should treat unknown `type` values as ignorable; the
`message` field is human-readable, not machine-parseable.

## Verification

Every implementation wave was gated by three independent verification
models (GLM-5.3, Gemini 3.8, Claude Opus) — zero P0 findings across
the run; the final closure gate returned SHIP. All 64 snapshot
goldens byte-identical through the pure-refactor phases; 464/464
tests; `clippy --all-targets -D warnings` clean; dependencies,
features, and MSRV untouched.
