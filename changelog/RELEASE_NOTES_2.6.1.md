# Release Notes - Version 2.6.1

**Release Date**: 2026-09-26

## Bug fixes

### Flaky e2e tests: temp-home collision (incident #37)

Parallel `cargo test` threads could construct two `TestEnv`s with the
same temp directory: `SystemTime::as_nanos()` returns identical values
when the calls land in the same clock tick. The two tests then shared
config, registry, and download directories, and whichever test finished
first ran cleanup `remove_dir_all` on the shared home — deleting the
other test's downloads mid-flight. This produced the sporadic
`No such file or directory` failures seen on the v2.6.0 tag CI
(Windows) and locally (~1 in 2–10 full-suite runs under load).

Temp-home suffixes now fold in a process-unique atomic counter
(post-fix: 20/20 full-suite runs green).

### Download robustness

- **Rename retry**: the final `.incomplete → final` rename is retried
  with backoff (5 attempts, 100ms·n) on transient filesystem locks —
  Windows antivirus/indexers can briefly hold a just-written file open
  (ERROR_SHARING_VIOLATION / ERROR_ACCESS_DENIED), which previously
  failed an otherwise complete download.
- **No more zombie chunk tasks**: on a chunk failure, remaining chunk
  tasks are no longer abandoned un-awaited; zombies could keep writing
  across the retry loop's delete/recreate of the `.incomplete` file.

### Test diagnostics (e2e harness)

Failure asserts now dump the child's stdout and stderr (JSON-mode error
events go to stdout — stderr-only messages were blind), the event types
present plus raw stdout when an expected event is missing, and the
directory contents when an expected file is missing. These turned
incident #37 from three blind CI failures into a same-day root cause.

No user-facing behavior changes beyond the download-robustness fixes.
