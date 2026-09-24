# Release Notes - Version 2.1.0

**Release Date**: 2026-09-24

## Verification overhaul: truthful progress + faster hashing

### Problem
After downloading large models (>20 GiB shards), SHA-256 verification looked broken:
the progress bar sat frozen near 0-1% for the entire run and only snapped to 100% at
the end. Additionally, hashing ran measurably below the hardware ceiling.

### Root causes found
1. **Progress-accounting bug**: the shared `verified_bytes` counter was advanced with
   `fetch_add(bytes_read)` only at every `update_interval`-th checkpoint, crediting just
   the last chunk - the published progress moved at 1/100th of the real speed.
2. **Per-read async dispatch**: every buffer read went through `tokio::fs`, i.e. one
   blocking-pool hop per read (~160k hops for a 20 GiB file), costing ~12% throughput.
3. **Undersized defaults**: 128 KiB read buffer and only 2 concurrent verifications.

### Changes
- **Fixed**: progress checkpoints now `store()` the exact running total; the gauge also
  shows throughput (GB/s) and a truthful ETA
- **Performance**: read+hash loop moved onto a single `spawn_blocking` thread with sync
  `std::fs` reads - measured 1.87 -> 2.10 GiB/s warm cache (the SHA-NI hardware ceiling
  on this class of CPU; sha2 0.10.9 with runtime SHA-NI detection matches OpenSSL)
- **Performance**: default `verification_buffer_size` 128 KiB -> 1 MiB
- **Performance**: default `concurrent_verifications` 2 -> 4 - multi-shard models verify
  in parallel; single-file SHA-256 remains non-parallelizable by design (chained
  compression function)
- **Tests**: regression tests for the progress accounting (mid-run high-water mark that
  fails under the old `fetch_add` code) and exact-digest end-to-end check

### Also in this release train (from 2.0.0 on this branch)
- 15-test insta snapshot suite for the render layer; options-popup collision fix and
  ellipsis truncation for long shard filenames
- Breaking removal of the headless/CLI mode (see 2.0.0 notes)

### Files Modified
`src/verification.rs`, `src/ui/render.rs`, `src/models.rs`, `src/config.rs` (test),
`Cargo.toml`, `changelog/`, `README.md`

**No Breaking Changes** relative to 2.0.0.
