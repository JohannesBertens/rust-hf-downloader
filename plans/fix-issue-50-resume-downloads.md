# Plan: Resumable downloads (issue #50)

**Issue:** https://github.com/JohannesBertens/rust-hf-downloader/issues/50 —
"Download does not resume if the download fails halfway through". Any network
interruption (timeout, connection reset) or terminal error mid-download
destroys all partial progress; every attempt restarts from byte 0, making
large models impossible to download on slow/flaky connections.

**Reproducing tests:** `tests/resume.rs` (added first, failing on `main`).

## 1. Root cause (three compounding sites)

| # | Site | Behavior |
|---|------|----------|
| 1 | `download::prepare_download_paths` (`src/download/mod.rs`) | Deletes any pre-existing `.incomplete` file at download start ("restart from beginning"). Cross-run progress destroyed. |
| 2 | `download::execute_download_with_retry` (`src/download/mod.rs`) | On every **transient** retry (timeout/connect) it deletes `.incomplete` → in-run progress destroyed. On terminal failure and 401 it also deletes → nothing survives for a later run. |
| 3 | `download::download_chunked` (`src/download/chunked.rs`) | Opens `.incomplete` with `truncate(true)`, preallocates with `set_len(total)`, and spawns every chunk from its byte-0 start. No `Range`-resume logic exists anywhere (documented as a GAP in `tests/download_failures.rs`). |

Additionally, chunks write **out-of-order at offsets** into one preallocated
file, so resuming requires knowing *which byte ranges are valid* — state that
exists nowhere today (the registry's single `downloaded_size: u64` cannot
represent holes).

## 2. Design: a per-file resume journal next to the `.incomplete` file

A small sidecar **journal** (`<final>.incomplete.state.json`, same directory
as `<final>.incomplete`) records everything needed to validate and resume:

```json
{
  "url": "https://huggingface.co/a/b/resolve/main/model.gguf",
  "revision": "main",
  "total_size": 100000,
  "chunk_size": 4096,
  "etag": "\"abc123\"",
  "chunk_offsets": [4096, 4096, 0, 0, 1696, ...]
}
```

- `chunk_offsets[i]` = bytes of chunk *i* already on disk (0 = not started,
  full chunk length = complete). Byte-granular resume within chunks.
- **Validation** before trusting a journal (all must hold, else discard both
  journal and `.incomplete` and restart fresh):
  - `url` and `revision` match this download's resolve URL/revision;
  - `total_size` matches the freshly probed size;
  - `chunk_size` matches the freshly computed chunk layout (config could have
    changed between runs);
  - `etag` matches when both journal and probe have one (server content
    changed otherwise). If either side lacks an ETag, size+url validation is
    the contract (the SHA256 verification pass remains the final backstop
    for same-size content changes).
  - every `chunk_offsets[i]` ≤ its chunk length (defensive: clamp out-of-range
    journals by discarding, not by partial trust).

**Why a sidecar (not the registry):** the journal must live and die with the
data file (same directory, atomic to validate, one journal per file), while
the registry is a global bookkeeping file whose `downloaded_size` is a coarse
scalar. The registry's semantics (`mark_failed` → `Incomplete`,
`downloaded_size = 0`) are unchanged by this fix — the journal is the resume
authority. No new registry ops, no lock-hierarchy changes (the journal writer
is download-internal state, not an `EngineState` field).

**Why not per-chunk part files (aria2-style):** resume would be trivial
(file length = progress), but the final assembly step would copy the entire
file (a 40 GB model → a 40 GB copy + transient double disk usage). The
single-preallocated-file + journal layout keeps the instant rename-to-final.

## 3. Changes by file

### 3.1 NEW `src/download/journal.rs` (private submodule behind the facade)

- `struct ResumeJournal { url, revision, total_size, chunk_size, etag, chunk_offsets }`
  (serde `Serialize`/`Deserialize`; `revision: Option<String>` normalized the
  same way registry metadata does — `None` for `DEFAULT_REVISION`).
- `journal_path(incomplete_or_final: &Path) -> PathBuf` — appends
  `.state.json` to the `.incomplete` path (i.e. `<final>.incomplete.state.json`).
- `load(path) -> Option<ResumeJournal>` (missing/corrupt → `None`).
- `save(path, &journal)` — atomic (same-dir temp + rename, the
  `utils::atomic_rename_with_retry` sync twin; `sync_all` before rename).
- `remove(path)` — best-effort delete.
- `ResumeJournal::validate(&self, url, total_size, chunk_size, etag) -> bool`
  — the §2 rules, as a pure function (unit-tested).
- `struct JournalWriter { inner: std::sync::Mutex<WriterInner>, path }` —
  shared into chunk tasks:
  - `record(chunk_id, downloaded)` — stores the offset and persists the
    journal **coalesced**: immediately on a chunk reaching completion or on
    first record; otherwise at most once per 1 s wall clock (an
    `Instant last_flush` inside the mutex). `try-lock`-free: a plain
    `lock()` — the critical section is a tiny JSON write with no await
    (std fs; brief blocking IO under a std mutex follows the registry
    precedent).
  - `snapshot() -> ResumeJournal` — for seeding progress/retry decisions.
- Crash-safety direction: data bytes are written **before** their journal
  update, so the journal can only *under*-report progress (re-downloading a
  few MB after a crash is safe); it can never point at bytes that were never
  requested. A power-loss window where journaled offsets outrun flushed data
  is theoretically possible (page-cache ordering) and intentionally accepted —
  the default-on SHA256 verification catches it, same as any mid-download
  corruption.

### 3.2 `src/download/chunked.rs`

- `probe_file_size` additionally returns the response `ETag` (if any).
- `download_chunked`:
  - computes the fresh layout (total_size, chunk_size, num_chunks) as today;
  - loads the journal and validates it (§2); on mismatch deletes journal +
    `.incomplete` and continues fresh (recreate + `truncate` + `set_len`);
  - on match: opens `.incomplete` **without** `truncate`, and only extends
    via `set_len(total_size)` (never shrinks a longer file — that would mean
    a foreign file: treat as mismatch);
  - seeds `DownloadProgress` (`downloaded = Σ offsets`,
    `chunk_completed[i] = offsets[i] == chunk_len(i)`) and the
    `progress_downloaded` atomic with the resumed byte count so HUD/CLI show
    true totals and speeds;
  - spawns only chunks with `offsets[i] < chunk_len(i)`; completed chunks
    are skipped entirely (no HTTP request);
  - `ChunkContext` gains `resume_offset: u64` (bytes already on disk for this
    chunk) and `journal: Option<Arc<JournalWriter>>`; the worker seeks to
    `start + resume_offset`, sends `bytes=(start+resume_offset)-stop`, and
    initializes its `chunk_downloaded` counter at `resume_offset`;
  - the worker calls `journal.record(chunk_id, chunk_downloaded)` on each
    progress tick (coalesced by the writer) and once more with the final
    chunk length on success;
  - after the final `atomic_rename_with_retry_async` to `<final>`:
    `journal::remove` (success consumes the journal).
- Zero-length/chunk-edge cases: a file whose journal says "everything
  complete" simply skips all chunks and renames (equivalent to a finished
  download interrupted before rename).

### 3.3 `src/download/mod.rs`

- `prepare_download_paths`: a pre-existing `.incomplete` is **kept** when a
  journal file sits next to it (status: "Resuming incomplete download for
  {file}"); the old delete-and-restart now only fires when there is *no*
  journal (orphans from older versions / foreign files), and it deletes the
  journal too if one exists but `.incomplete` is missing.
- `execute_download_with_retry`:
  - transient-retry arm: **keep** `.incomplete` + journal (status mentions
    resuming);
  - terminal-error and 401 arms: **keep** `.incomplete` + journal (the whole
    point of the issue — a later run, or a re-auth, resumes where it left
    off). Registry behavior (`mark_failed`) unchanged.
- Module docs updated (the facade doc currently promises `.incomplete`
  deletion on retries — now promises resume).

### 3.4 `src/ui/app/downloads.rs`

- `delete_incomplete_downloads`: also removes the journal next to each
  `.incomplete` file (best-effort), so the TUI delete flow doesn't leave
  orphan journals. (Its unit test gains a journal fixture assertion.)

### 3.5 Docs & pins

- `AGENTS.md` source-tree entry for `download/` — updated wording (resume
  journal instead of deletion-on-retry).
- `tests/download_failures.rs` — the two "nothing survives on disk"
  assertions flip to the new contract (`.incomplete` + journal preserved;
  registry/exit-code assertions unchanged); the coverage-table GAP note is
  replaced by the implemented resume scenarios.
- No lock-hierarchy changes (docs guard test unaffected); no registry op
  changes (golden fixtures unaffected).

## 4. Test matrix

- `tests/resume.rs` (added in the reproduce commit) — all 5 scenarios turn
  green with this design.
- New unit tests in `src/download/journal.rs`: save/load round-trip;
  validation matrix (url/revision/total_size/chunk_size/etag/offset-bounds,
  etag-absent fallback); writer coalescing (first record persists, interval
  records throttle, completion forces).
- Existing suites must stay green — in particular
  `timeout_then_success_retries_and_exits_ok` (success cleans up both files)
  and the attempt-count test (probe-per-attempt counting is unchanged).

## 5. Out of scope (explicit)

- No registry schema changes (`downloaded_size` stays coarse).
- No `If-Range` header (we re-validate via probe + journal header fields).
- No cross-process locking of the journal (same deferred status as the
  registry's cross-process story; one process per download dir is the
  existing model).
- No change to `hf-cache sync` publish/staging logic — it inherits the
  transport fix (its staging `.incomplete` files already survive failed runs
  "for resume value"; now they actually resume).
