# Release Notes - Version 2.12.0

**Release Date**: 2026-10-05

## New features

### Overall progress, speed, and ETA for multi-file downloads

Multi-file CLI runs (`download` with several files, `--all`, `--quant`
picking many parts, and `hf-cache sync`) now show **aggregate** progress
instead of only the current file's numbers. Human mode keeps the same
single rewritten line — aggregate first, active file demoted to name +
percent:

```text
[3/17 files 43% │ 12.63 GB/29.06 GB │ 88.0 MB/s eta 3m11s] ▸ model-00004-of-00017.safetensors 61%
```

- The aggregate percent is clamped at 100 %, and the line is now erased
  to end-of-line on each rewrite (`\x1b[K`), so shrinking fields (unit
  crossings, a vanishing ETA) no longer leave stale glyphs — single-file
  progress lines get the same fix.
- Single-file runs render exactly as before.

`--json` extends the `progress` event with an **optional** `overall`
object (`{ files_done, files_total, downloaded_bytes, total_bytes }`) on
multi-file runs — the additive-only NDJSON contract is unchanged, and the
existing `progress` snapshot is byte-identical. Files are downloaded
strictly serially, so the event's `speed_mbps` is the aggregate wire rate
and consumers can compute an overall ETA from `total_bytes -
downloaded_bytes`.

## Internal

- `RunTally` tracks `done_bytes` (bytes of finished files) alongside the
  per-file outcome counters; `poll_once` derives `OverallProgress` while
  holding only its pre-existing `try_lock`s, so the AGENTS.md lock
  hierarchy is untouched.
- UI render snapshots are now **version-stable**: an insta filter
  normalizes the footer version to `v<VERSION>`, so version bumps no
  longer rewrite 7+ TUI snapshots — the version lives only in
  `Cargo.toml`.
