# Release Notes - Version 2.13.0

**Release Date**: 2026-10-05

## New features

### `--progress plain` — progress output that works without a tty

A new `--progress <MODE>` flag on `download` and `hf-cache sync`
(`auto` | `plain` | `none`, default `auto`):

- **`plain`** prints one newline progress line every ~10 s on stderr,
  tty-independent — built for `docker run` without `-t`, CI logs, and
  `tee` pipelines, where `auto` is deliberately silent (no `\r`
  rewrites in piped output):

  ```text
  [3/36 files 43% │ 71.5/166.3 GB │ 88 MB/s eta 18m12s] ▸ tensors/model-00021-of-00036.safetensors 61%
  verifying: 2 in flight, 41 verified
  ```

- It also closes the **verification-drain gap**: after all downloads
  finish, SHA256 hashing previously produced no live output between
  `✓ verified` milestones. In `plain` mode a
  `verifying: N in flight, M verified` heartbeat shares the same ~10 s
  throttle window, so there is at most one progress line per interval
  for the whole run.
- **`auto`** (default) is unchanged: single-line `\r` rewrites on a
  tty, silence when piped. **`none`** disables human progress output
  entirely.

`--quiet` and `--json` still take precedence (quiet suppresses the
heartbeat; JSON consumers already have throttled `progress` events with
the `overall` aggregate).

## Internal

- Progress line rendering extracted into pure helpers
  (`format_file_progress`, `format_overall_progress`,
  `verification_heartbeat_line`) shared by the tty renderer and the
  plain-mode printer — unit-tested with exact-output assertions.
- The plain heartbeat is emitted from `poll_once` only when no file is
  actively downloading, reading the pre-existing
  `verification_progress` try-lock; no new locks, no engine changes.
