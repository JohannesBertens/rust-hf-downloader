//! The resume journal (issue #50): a small sidecar next to the
//! `.incomplete` file recording everything needed to validate and resume a
//! partially downloaded file — `<final>.incomplete.state.json`.
//!
//! # Why a sidecar (not the registry)
//!
//! The journal must live and die with the data file (same directory,
//! validated atomically, one journal per file). The registry's
//! `downloaded_size: u64` is a coarse scalar that cannot represent the
//! HOLES a parallel chunked download leaves in the preallocated
//! `.incomplete` file, so per-chunk byte offsets need their own home. The
//! registry is deliberately untouched by resume (see the plan:
//! `plans/fix-issue-50-resume-downloads.md`).
//!
//! # Layout
//!
//! `chunk_offsets[i]` is the number of bytes of chunk *i* already on disk
//! (0 = not started, chunk length = complete). Byte-granular resume within
//! chunks is the point: with 5–100 MB chunks, boolean chunk completion
//! would forfeit hours of progress on the slow connections this exists for.
//!
//! # Validation ([`ResumeJournal::validate`])
//!
//! A journal is trusted only when EVERY rule holds — url (embeds the
//! revision), total_size, chunk_size, offsets length, per-offset bounds,
//! on-disk `.incomplete` length, sha256 (primary, when both sides have
//! one), etag (secondary, when both sides have one). Any mismatch discards
//! journal + `.incomplete` and restarts fresh: fail-safe direction, never
//! a partial trust.
//!
//! # Crash safety
//!
//! Data bytes are written before their journal update, so the journal can
//! only UNDER-report progress (a safe re-download); the completion record
//! additionally happens after the chunk file handle was `flush` +
//! `sync_all`ed, making completed chunks power-loss safe. Mid-chunk
//! offsets keep a theoretical page-cache window (accepted; SHA256
//! verification backstops LFS files).
//!
//! # Lock discipline (TERMINAL LEAF)
//!
//! [`JournalWriter`]'s `std::sync::Mutex` is a terminal leaf exactly like
//! the registry's `REGISTRY_WRITE`: nothing else may be acquired while it
//! is held and no `.await` may happen inside (the critical section is a
//! tiny synchronous JSON write). Callers (chunk tasks) call
//! [`JournalWriter::record`] OUTSIDE any `progress.lock().await` scope —
//! the reverse order (journal lock → progress lock) is forbidden by
//! construction. The lock is poison-tolerant (`into_inner`, the registry
//! pattern) so one panicked chunk cannot cascade to its siblings.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The journal JSON document persisted next to the `.incomplete` file.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct ResumeJournal {
    /// The resolve URL this download's bytes come from. Embeds the
    /// revision (`resolve_url` builds `{base}/{model}/resolve/{rev}/{file}`),
    /// so url equality subsumes revision equality.
    pub url: String,
    /// Diagnostics only (the url is the validated field): the git revision
    /// the URL points at, normalized like registry metadata (`None` for the
    /// default revision).
    pub revision: Option<String>,
    /// Total file size as probed when the journal was written.
    pub total_size: u64,
    /// The chunk layout's chunk size (offsets are meaningless under a
    /// different layout).
    pub chunk_size: u64,
    /// The probe response's ETag, when the server sent one (secondary
    /// validator — real-CDN ETags may vary per node, so a mismatch here is
    /// only advisory; the sha256 check is the load-bearing one).
    pub etag: Option<String>,
    /// The run's `expected_sha256` (LFS oid), when the tree API advertised
    /// one — the PRIMARY content validator: stable across runs, unlike
    /// CDN ETags.
    pub sha256: Option<String>,
    /// Per-chunk downloaded byte counts. Length == num_chunks.
    pub chunk_offsets: Vec<u64>,
}

/// How the journal coalesces periodic writes (chunk completions and the
/// first record always persist immediately).
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// The sidecar journal path for a `.incomplete` (or final) file path:
/// `<path>.state.json`.
pub(super) fn journal_path(incomplete_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.state.json", incomplete_path.display()))
}

/// Load a journal; `None` when missing or corrupt (never trusted blindly).
pub(super) fn load(path: &Path) -> Option<ResumeJournal> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Atomically persist a journal (same-dir temp + `sync_all` + rename).
/// Best-effort: IO failures are swallowed by [`JournalWriter::record`]
/// callers that use it through the writer; direct callers log upstream.
pub(super) fn save(path: &Path, journal: &ResumeJournal) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(journal)
        .map_err(|e| std::io::Error::other(format!("journal serialize: {e}")))?;
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&tmp, &body)?;
    if let Ok(file) = std::fs::File::open(&tmp) {
        // Best-effort fsync of the temp body before the rename makes it
        // visible (power-loss window shrunk to metadata-only).
        let _ = file.sync_all();
    }
    std::fs::rename(&tmp, path)
}

/// Best-effort journal delete (missing file is fine).
pub(super) fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

impl ResumeJournal {
    /// Fresh all-zero journal for a download that is about to start.
    pub(super) fn fresh(
        url: &str,
        revision: Option<String>,
        total_size: u64,
        chunk_size: u64,
        etag: Option<String>,
        sha256: Option<String>,
        num_chunks: usize,
    ) -> Self {
        Self {
            url: url.to_string(),
            revision,
            total_size,
            chunk_size,
            etag,
            sha256,
            chunk_offsets: vec![0; num_chunks],
        }
    }

    /// Length of chunk `i` under this journal's layout.
    fn chunk_len(&self, i: usize) -> u64 {
        self.chunk_size
            .min(self.total_size - i as u64 * self.chunk_size)
    }

    /// The §-validation rules (see module docs). `incomplete_len` is the
    /// on-disk `.incomplete` length (`None` when the file is absent).
    /// Pure; unit-tested below.
    pub(super) fn validate(
        &self,
        url: &str,
        total_size: u64,
        chunk_size: u64,
        etag: Option<&str>,
        sha256: Option<&str>,
        incomplete_len: Option<u64>,
    ) -> bool {
        if self.url != url || self.total_size != total_size || self.chunk_size != chunk_size {
            return false;
        }
        // A truncated/longer offsets vector would misalign chunk ids onto
        // wrong byte ranges — corruption, not just lost progress.
        if self.chunk_offsets.len() != total_size.div_ceil(chunk_size) as usize {
            return false;
        }
        let mut highest_byte = 0u64;
        for (i, &offset) in self.chunk_offsets.iter().enumerate() {
            if offset > self.chunk_len(i) {
                return false;
            }
            if offset > 0 {
                highest_byte = highest_byte.max(i as u64 * chunk_size + offset);
            }
        }
        // The data file must actually contain every byte the journal
        // claims (an externally truncated `.incomplete` must not resume
        // into a zero-hole).
        match incomplete_len {
            Some(len) if len >= highest_byte => {}
            Some(_) => return false,
            // No data file: nothing to resume (the journal alone is not
            // progress). Callers treat this as "restart fresh".
            None => return false,
        }
        // Primary content validator: the stable LFS oid, when both sides
        // have one.
        if let (Some(journaled), Some(current)) = (&self.sha256, sha256) {
            if journaled != current {
                return false;
            }
        }
        // Secondary, best-effort: ETag when both sides have one.
        if let (Some(journaled), Some(current)) = (&self.etag, etag) {
            if journaled != current {
                return false;
            }
        }
        true
    }
}

struct WriterInner {
    journal: ResumeJournal,
    dirty: bool,
    last_flush: Option<Instant>,
}

/// The shared, coalescing journal writer handed to every chunk task.
///
/// `record` is called from async chunk workers but is fully synchronous
/// (terminal-leaf lock; see module docs). Write failures are silently
/// swallowed — a journal IO hiccup must never fail an otherwise healthy
/// download; the worst case is a lost resume point, not a lost download.
pub(super) struct JournalWriter {
    path: PathBuf,
    inner: Mutex<WriterInner>,
}

impl JournalWriter {
    pub(super) fn new(path: PathBuf, journal: ResumeJournal) -> Self {
        Self {
            path,
            inner: Mutex::new(WriterInner {
                journal,
                dirty: false,
                last_flush: None,
            }),
        }
    }

    /// Record a chunk's downloaded byte count, persisting coalesced:
    /// immediately on completion (offset == chunk length) or the first
    /// record; otherwise at most once per [`FLUSH_INTERVAL`].
    pub(super) fn record(&self, chunk_id: usize, downloaded: u64) {
        let mut inner = self.lock_inner();
        let completed = downloaded >= inner.journal.chunk_len(chunk_id);
        if let Some(offset) = inner.journal.chunk_offsets.get_mut(chunk_id) {
            // Monotonic: a late zombie record (a task aborted inside a
            // previous `record` call finishing after abort_all) can only
            // under-report — never let it rewind progress.
            if downloaded < *offset {
                return;
            }
            *offset = downloaded;
        } else {
            return;
        }
        let now = Instant::now();
        let due = match inner.last_flush {
            None => true,
            Some(last) => now.duration_since(last) >= FLUSH_INTERVAL,
        };
        if completed || due {
            let journal = inner.journal.clone();
            inner.last_flush = Some(now);
            inner.dirty = false;
            // Terminal leaf: sync write while holding ONLY this mutex.
            let _ = save(&self.path, &journal);
        } else {
            inner.dirty = true;
        }
    }

    /// Flush any un-persisted offset state (best-effort).
    pub(super) fn flush(&self) {
        let mut inner = self.lock_inner();
        if inner.dirty || inner.last_flush.is_none() {
            let journal = inner.journal.clone();
            inner.last_flush = Some(Instant::now());
            inner.dirty = false;
            let _ = save(&self.path, &journal);
        }
    }

    /// Current offsets snapshot (progress seeding / logging).
    #[cfg(test)]
    pub(super) fn snapshot(&self) -> ResumeJournal {
        self.lock_inner().journal.clone()
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, WriterInner> {
        // Poison-tolerant (the REGISTRY_WRITE pattern): a panicking chunk
        // task must not cascade-panic its siblings.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod journal_tests {
    //! Pure-logic pins for the resume journal: save/load round-trip, the
    //! full validation matrix, writer coalescing/monotonicity, and poison
    //! tolerance.

    use super::*;

    fn journal() -> ResumeJournal {
        ResumeJournal {
            url: "http://x/a/b/resolve/main/model.gguf".to_string(),
            revision: None,
            total_size: 10_000,
            chunk_size: 4_000,
            etag: Some("\"v1\"".to_string()),
            sha256: Some("cafebabe".to_string()),
            chunk_offsets: vec![4_000, 2_000, 0],
        }
    }

    fn tmp_path(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("rhd-journal-{tag}-{}-{n}", std::process::id()))
    }

    #[test]
    fn save_load_round_trip() {
        let path = tmp_path("roundtrip");
        let j = journal();
        save(&path, &j).unwrap();
        assert_eq!(load(&path), Some(j.clone()));
        remove(&path);
        assert_eq!(load(&path), None);
    }

    #[test]
    fn load_rejects_corrupt_json() {
        let path = tmp_path("corrupt");
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load(&path), None);
        remove(&path);
    }

    #[test]
    fn validate_accepts_matching_journal() {
        assert!(journal().validate(
            "http://x/a/b/resolve/main/model.gguf",
            10_000,
            4_000,
            Some("\"v1\""),
            Some("cafebabe"),
            Some(6_000),
        ));
    }

    #[test]
    fn validate_matrix_rejects_every_mismatch() {
        let j = journal();
        let url = "http://x/a/b/resolve/main/model.gguf";
        let cases: Vec<(&str, bool)> = vec![
            (
                "wrong url",
                j.validate("http://x/other", 10_000, 4_000, None, None, Some(6_000)),
            ),
            (
                "wrong total_size",
                j.validate(url, 10_001, 4_000, None, None, Some(6_000)),
            ),
            (
                "wrong chunk_size",
                j.validate(url, 10_000, 4_001, None, None, Some(6_000)),
            ),
            (
                "etag mismatch",
                j.validate(url, 10_000, 4_000, Some("\"v2\""), None, Some(6_000)),
            ),
            (
                "sha256 mismatch",
                j.validate(url, 10_000, 4_000, None, Some("deadbeef"), Some(6_000)),
            ),
            (
                "incomplete file too short",
                j.validate(url, 10_000, 4_000, None, None, Some(5_999)),
            ),
            (
                "incomplete file missing",
                j.validate(url, 10_000, 4_000, None, None, None),
            ),
        ];
        for (name, ok) in cases {
            assert!(!ok, "validate must reject: {name}");
        }
    }

    #[test]
    fn validate_etag_absent_on_either_side_falls_back_to_the_rest() {
        // Journal has an etag, the new probe does not (or vice versa): the
        // etag rule is skipped, the other checks govern.
        assert!(journal().validate(url_of(), 10_000, 4_000, None, Some("cafebabe"), Some(6_000)));
        let mut no_etag = journal();
        no_etag.etag = None;
        assert!(no_etag.validate(url_of(), 10_000, 4_000, Some("\"v9\""), None, Some(6_000)));
    }

    #[test]
    fn validate_rejects_truncated_offsets_vector() {
        let mut j = journal();
        j.chunk_offsets = vec![4_000]; // would misalign chunk ids
        assert!(!j.validate(url_of(), 10_000, 4_000, None, None, Some(6_000)));
    }

    #[test]
    fn validate_rejects_out_of_range_offset() {
        let mut j = journal();
        j.chunk_offsets[2] = 4_001; // > last chunk length (2_000)
        assert!(!j.validate(url_of(), 10_000, 4_000, None, None, Some(10_001)));
    }

    #[test]
    fn validate_file_length_covers_exactly_the_highest_claimed_byte() {
        // chunk 1 claims 2_000 bytes at offset 4_000 -> highest byte 6_000.
        assert!(journal().validate(url_of(), 10_000, 4_000, None, None, Some(6_000)));
        assert!(!journal().validate(url_of(), 10_000, 4_000, None, None, Some(5_999)));
    }

    fn url_of() -> &'static str {
        "http://x/a/b/resolve/main/model.gguf"
    }

    #[test]
    fn downloaded_bytes_sums_offsets() {
        assert_eq!(journal().chunk_offsets.iter().sum::<u64>(), 6_000);
    }

    #[test]
    fn writer_first_record_and_completion_persist_immediately() {
        let path = tmp_path("writer");
        let writer = JournalWriter::new(
            path.clone(),
            ResumeJournal::fresh(url_of(), None, 10_000, 4_000, None, None, 3),
        );
        writer.record(0, 100);
        assert_eq!(load(&path).unwrap().chunk_offsets, vec![100, 0, 0]);
        writer.record(1, 4_000); // completion forces a flush
        assert_eq!(load(&path).unwrap().chunk_offsets, vec![100, 4_000, 0]);
        remove(&path);
    }

    #[test]
    fn writer_periodic_records_coalesce_and_flush_persists() {
        let path = tmp_path("coalesce");
        let writer = JournalWriter::new(
            path.clone(),
            ResumeJournal::fresh(url_of(), None, 10_000, 4_000, None, None, 3),
        );
        writer.record(0, 1); // first record persists (last_flush = now)
        writer.record(0, 2_000); // within the interval: memory only
        assert_eq!(load(&path).unwrap().chunk_offsets[0], 1);
        assert_eq!(writer.snapshot().chunk_offsets[0], 2_000);
        writer.flush();
        assert_eq!(load(&path).unwrap().chunk_offsets[0], 2_000);
        remove(&path);
    }

    #[test]
    fn writer_records_are_monotonic_per_chunk() {
        let path = tmp_path("monotonic");
        let writer = JournalWriter::new(
            path.clone(),
            ResumeJournal::fresh(url_of(), None, 10_000, 4_000, None, None, 3),
        );
        writer.record(0, 3_000);
        // A late zombie record (aborted task finishing after abort_all)
        // must not rewind progress.
        writer.record(0, 10);
        assert_eq!(writer.snapshot().chunk_offsets[0], 3_000);
        remove(&path);
    }

    #[test]
    fn writer_survives_poisoned_lock() {
        let path = tmp_path("poison");
        let writer = JournalWriter::new(
            path.clone(),
            ResumeJournal::fresh(url_of(), None, 10_000, 4_000, None, None, 3),
        );
        // Simulate a panicked chunk task poisoning the mutex: take the
        // lock and panic while holding it.
        let inner = &writer.inner;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = inner.lock().unwrap();
            panic!("simulated chunk panic under the journal lock");
        }));
        // Subsequent records still work (poison-tolerant into_inner).
        writer.record(2, 2_000);
        assert_eq!(writer.snapshot().chunk_offsets[2], 2_000);
        remove(&path);
    }
}
