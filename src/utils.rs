//! Exactly two genuinely-generic helper families (final cohesion pass —
//! the formatting delegates moved to their single home [`crate::fmt`]):
//!
//! - **Digest streaming**: [`stream_file_digest`] (one buffered
//!   read+hash loop with per-chunk callbacks) plus [`sha256_file`] and
//!   [`DIGEST_CHUNK`] — used by the verification worker and the hub-cache
//!   layout writer.
//! - **Atomic rename**: [`atomic_rename_with_retry`] and its async twin
//!   [`atomic_rename_with_retry_async`] — the shared final-rename
//!   primitive whose retry policy is chosen per call site (the download
//!   pipeline retries transient filesystem locks; other sites pass
//!   `retries = 0` for a plain single-attempt rename).
//!
//! Nothing else belongs here — both families are generic over their
//! callers (no UI, CLI, engine or registry concepts appear below).

use sha2::Digest as _;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// Read granularity of [`sha256_file`] and the hub-cache blob hasher.
/// 64 KiB amortizes syscall overhead well below the SHA-NI hashing
/// ceiling; callers with progress-reporting needs pick their own size via
/// [`stream_file_digest`].
pub(crate) const DIGEST_CHUNK: usize = 64 * 1024;

/// Streams `path` through `hasher` in chunks of `buffer_size` bytes,
/// calling `on_chunk(chunk, bytes_hashed_so_far)` after every non-empty
/// read (the same loop the verification worker used to inline: open,
/// buffered read, hash, report). Returns the number of bytes hashed, so
/// callers that need a stat-vs-read consistency check (see
/// [`crate::cache_layout::git_blob_sha1`]) can detect a file that changed
/// size while being read.
///
/// A `buffer_size` of 0 is legal: the reads then bypass buffering one
/// byte at a time (a zero-length read would otherwise read as EOF, since
/// `Read` on an empty slice is always `Ok(0)`).
pub(crate) fn stream_file_digest<H, F>(
    path: &Path,
    hasher: &mut H,
    buffer_size: usize,
    mut on_chunk: F,
) -> std::io::Result<u64>
where
    H: sha2::digest::Update,
    F: FnMut(&[u8], u64),
{
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::with_capacity(buffer_size, file);
    let mut buffer = vec![0u8; buffer_size.max(1)];
    let mut hashed: u64 = 0;
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
        hashed += bytes_read as u64;
        on_chunk(&buffer[..bytes_read], hashed);
    }
    Ok(hashed)
}

/// Streaming SHA-256 of a file's contents, hex-encoded (BufReader with a
/// fixed [`DIGEST_CHUNK`] buffer — the whole file is never held in
/// memory). Files that change while being hashed produce whatever digest
/// the interleaved reads saw; callers that must detect that race should
/// use [`stream_file_digest`] and compare bytes read against a stat taken
/// up front.
// 2026-10 (W1.6): no production caller yet — verification streams with
//  progress callbacks and `update.rs` hashes network chunks — so this is
//  pinned only by the known-vector tests below until a whole-file
//  SHA-256 consumer appears.
#[cfg_attr(not(test), allow(dead_code))]
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut hasher = sha2::Sha256::new();
    stream_file_digest(path, &mut hasher, DIGEST_CHUNK, |_, _| {})?;
    Ok(hex::encode(hasher.finalize()))
}

/// Rename with bounded retry for transient filesystem locks.
///
/// Windows antivirus/indexers can hold a just-written file open for a short
/// window (ERROR_SHARING_VIOLATION = 32, ERROR_ACCESS_DENIED = 5); a single
/// such window must not fail a fully-downloaded file. `retries` counts the
/// attempts after the first (total attempts = `retries + 1`); each retry
/// sleeps `delay` scaled by the retry number (linear backoff). Only
/// transient-looking lock errors (see [`is_transient_fs_lock`]) are
/// retried — anything else is returned immediately, and `retries = 0` is
/// an exact single-attempt `std::fs::rename`.
pub fn atomic_rename_with_retry(
    src: &Path,
    dst: &Path,
    retries: u32,
    delay: Duration,
) -> std::io::Result<()> {
    for attempt in 0..=retries {
        match std::fs::rename(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < retries && is_transient_fs_lock(&e) => {
                std::thread::sleep(delay * (attempt + 1));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("retry loop always returns")
}

/// Whether an IO error looks like a transient lock by another process
/// (sharing violation or access denied). On Unix these kinds usually
/// indicate real permission problems, but a handful of retries is
/// harmless there.
fn is_transient_fs_lock(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::PermissionDenied) || e.raw_os_error() == Some(32)
}

/// Async twin of [`atomic_rename_with_retry`]: identical retry policy
/// (same attempt semantics, same transient-lock predicate, same linear
/// backoff), but the rename runs via `tokio::fs::rename` on the blocking
/// pool and the backoff sleeps are `tokio::time::sleep` — so an async
/// caller (the download transport) never blocks its Tokio worker thread
/// during Windows AV/indexer lock contention (regression review P1: the
/// sync twin did, stalling cooperative tasks for up to ~1s).
/// Sync-context callers (the hf-cache layout writer, whose legacy path
/// always used `std::fs::rename`) keep the sync twin.
pub async fn atomic_rename_with_retry_async(
    src: &Path,
    dst: &Path,
    retries: u32,
    delay: Duration,
) -> std::io::Result<()> {
    for attempt in 0..=retries {
        match tokio::fs::rename(src, dst).await {
            Ok(()) => return Ok(()),
            Err(e) if attempt < retries && is_transient_fs_lock(&e) => {
                tokio::time::sleep(delay * (attempt + 1)).await;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("retry loop always returns")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rhd-utils-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn rename_moves_file_with_retry_budget() {
        let dir = tmp("rename-ok");
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        std::fs::write(&src, b"payload").expect("write src");
        atomic_rename_with_retry(&src, &dst, 4, Duration::from_millis(1)).expect("rename");
        assert_eq!(std::fs::read(&dst).unwrap(), b"payload");
        assert!(!src.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_zero_retries_is_plain_single_attempt() {
        // retries = 0 is the policy cache_layout's rename_replacing
        // delegates with: one attempt, success or the raw error.
        let dir = tmp("rename-zero");
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        std::fs::write(&src, b"x").expect("write src");
        atomic_rename_with_retry(&src, &dst, 0, Duration::from_secs(30)).expect("rename");
        assert!(dst.is_file());
        let missing = dir.join("missing.bin");
        let err = atomic_rename_with_retry(&missing, &dst, 0, Duration::from_secs(30)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_returns_non_transient_errors_without_sleeping() {
        // NotFound is never transient: even with a 30s base delay the call
        // must return immediately instead of burning the retry budget.
        let start = std::time::Instant::now();
        let err = atomic_rename_with_retry(
            Path::new("rhd-no-such-src/nope.bin"),
            Path::new("rhd-no-such-dst/nope.bin"),
            4,
            Duration::from_secs(30),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    // ---- streaming digest helpers ----

    /// Deterministic pseudo-random payload (xorshift64*) so multi-chunk
    /// vectors never depend on RNG state.
    fn pseudo_random(len: usize) -> Vec<u8> {
        let mut state = 0x9E3779B97F4A7C15u64;
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            out.extend_from_slice(&state.to_le_bytes());
        }
        out.truncate(len);
        out
    }

    fn sha256_one_shot(bytes: &[u8]) -> String {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(bytes))
    }

    fn write_tmp_file(dir_tag: &str, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = tmp(dir_tag);
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write temp file");
        path
    }

    #[test]
    fn sha256_file_known_vectors() {
        // Reference vectors for the bare content hash (no git blob header).
        let empty = write_tmp_file("digest-empty", "empty", b"");
        assert_eq!(
            sha256_file(&empty).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let hello = write_tmp_file("digest-hello", "hello", b"hello\n");
        assert_eq!(
            sha256_file(&hello).unwrap(),
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
        );
        let _ = std::fs::remove_dir_all(empty.parent().unwrap());
        let _ = std::fs::remove_dir_all(hello.parent().unwrap());
    }

    #[test]
    fn sha256_file_hashes_multi_chunk_files_exactly() {
        // 1 MiB + 1 B: strictly larger than the 64 KiB digest buffer and
        // not an exact multiple of it, so the loop must handle a short
        // final chunk.
        let payload = pseudo_random(1024 * 1024 + 1);
        let path = write_tmp_file("digest-big", "big.bin", &payload);
        assert_eq!(sha256_file(&path).unwrap(), sha256_one_shot(&payload));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stream_digest_zero_inner_buffer_is_still_exact() {
        // Capacity-0 BufReader: reads bypass the (empty) internal buffer,
        // and the loop must not misread a 1-byte read as EOF. Byte-at-a-
        // time, so kept small.
        let payload = pseudo_random(1024);
        let path = write_tmp_file("digest-zero", "z.bin", &payload);
        let mut hasher = sha2::Sha256::new();
        let hashed = stream_file_digest(&path, &mut hasher, 0, |_, _| {}).unwrap();
        assert_eq!(hashed, payload.len() as u64);
        assert_eq!(hex::encode(hasher.finalize()), sha256_one_shot(&payload));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stream_digest_reports_running_totals_per_chunk() {
        // The on_chunk callback sees each non-empty chunk and the exact
        // running total (the invariant the verification worker's progress
        // accounting relies on).
        let payload = pseudo_random(10);
        let path = write_tmp_file("digest-cb", "cb.bin", &payload);
        let mut hasher = sha2::Sha256::new();
        let mut seen = Vec::new();
        stream_file_digest(&path, &mut hasher, 3, |chunk, total| {
            seen.push((chunk.len() as u64, total));
        })
        .unwrap();
        assert_eq!(
            seen,
            vec![(3, 3), (3, 6), (3, 9), (1, 10)],
            "chunks of 3 over 10 bytes with exact running totals"
        );
        assert_eq!(hex::encode(hasher.finalize()), sha256_one_shot(&payload));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

#[cfg(test)]
mod async_rename_tests {
    use super::*;

    #[tokio::test]
    async fn async_rename_succeeds_first_try_and_moves_the_file() {
        let dir = std::env::temp_dir().join(format!("rhd-utils-aren-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        std::fs::write(&src, b"payload").expect("write");
        atomic_rename_with_retry_async(&src, &dst, 4, Duration::from_millis(1))
            .await
            .expect("rename");
        assert!(dst.exists() && !src.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn async_rename_propagates_non_transient_errors_without_retry() {
        // Renaming a nonexistent source is NotFound (not a transient lock):
        // must fail fast with the raw error.
        let err = atomic_rename_with_retry_async(
            std::path::Path::new("/nonexistent/rhd-src"),
            std::path::Path::new("/nonexistent/rhd-dst"),
            4,
            Duration::from_millis(1),
        )
        .await
        .expect_err("must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
