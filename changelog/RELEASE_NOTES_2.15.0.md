# Release Notes - Version 2.15.0

**Release Date**: 2026-10-13

Fixes [issue #50](https://github.com/JohannesBertens/rust-hf-downloader/issues/50):
"Download does not resume if the download fails halfway through."
Before this release, any interruption — network timeout, connection
reset, terminal HTTP error, an expired token, even Ctrl-C followed by a
re-run — deleted the partial `.incomplete` file and restarted the
download from byte 0, making large models effectively impossible to
download on slow or flaky connections. 464 → 487 tests.

## User-visible changes

- **Interrupted downloads now RESUME (issue #50, PR #51)**:
  - Transient network errors (timeouts, connection resets) retry and
    continue from where they left off — no more restart-from-zero on
    every hiccup.
  - Terminal failures, 401/authorization errors and SIGINT keep the
    partial download on disk: a later run (after fixing the network,
    re-authenticating, or simply re-running) picks up the missing byte
    ranges instead of re-downloading everything.
  - Resume state is a per-file sidecar journal
    (`<file>.incomplete.state.json`, byte-granular per-chunk offsets)
    that is validated against the server on every attempt: URL, size,
    chunk layout, the LFS `sha256` (primary — stable across runs) and
    the ETag (secondary, only when no LFS hash exists). If the server's
    file changed (different size, hash, or ETag), the journal is
    discarded and the download restarts cleanly — resume never
    continues into the wrong bytes.
  - Works across all frontends: CLI `download`, `hf-cache sync`
    (staging inherits the transport fix), and the TUI. The TUI's
    "delete incomplete downloads" flow removes the journal too.
  - The interrupted-download hint (`exit 130` message, CLI output) now
    says partial downloads resume on the next run.
- **Transport correctness hardening** (found during the fix's
  multi-model review; closes a pre-existing silent-corruption hole):
  - A server response that ignores a `Range` request (HTTP `200`
    instead of `206`) now fails the chunk with a clear error instead of
    writing the full body at the wrong offset.
  - Short stream bodies (a connection that ends before delivering the
    requested range) and over-delivering bodies (more bytes than the
    requested range) now fail the chunk instead of being miscounted as
    success — previously either could silently corrupt the downloaded
    file in ways the file-size check could not see.

## Internal

- New `download::journal` module: the resume journal and its writer —
  a terminal-leaf, poison-tolerant mutex (nothing else acquired under
  it, no awaits inside, records happen outside every other lock
  scope), coalesced atomic persistence (same-dir temp + fsync +
  rename-with-retry; first record and chunk completions immediate,
  otherwise at most once per second), strict data-before-journal crash
  ordering (the journal can only under-report progress — a crash
  re-downloads a bounded amount, never resumes into wrong bytes), and
  completion records after `flush` + `fsync` so completed chunks are
  power-loss safe even for files without a verification hash.
- Progress/speed counters are seeded with resumed bytes, so HUD and
  CLI progress show true totals and speeds after a resume (resumed
  bytes are not counted as fresh throughput).
- Registry semantics, ops and schema untouched; engine lock hierarchy
  untouched (docs guard still green).
- Tests 464 → 487: five wire-level resume e2e scenarios (in-run retry
  resume, terminal-failure keep, cross-run resume, server size-change
  discard, server content-change discard), 17 journal unit tests
  (validation matrix, coalescing, monotonicity, poison tolerance), and
  four worker-level tests including mid-chunk resume — the core
  mechanism — plus 206/short-stream/over-delivery rejection against an
  in-process HTTP server.
- Process: reproduction tests first (all five failing on the old
  transport), then plan, plan review by three models, implementation,
  and fix review by the same three models (GLM-5.3, GLM-5.2,
  GLM-5.3-flash — all APPROVE-WITH-CHANGES; every finding applied,
  including one unanimous lock-discipline finding). Review artifacts
  in `.review/`; plan in `plans/fix-issue-50-resume-downloads.md`.
