//! Small shared helpers: [`format_size`] and [`format_number`] are thin
//! aliases for the shared surface-pinned formatters in [`crate::fmt`]
//! (kept because they are used across the UI and CLI), and
//! [`atomic_rename_with_retry`] is the shared final-rename primitive whose
//! retry policy is chosen per call site (the download pipeline retries
//! transient filesystem locks; other sites pass `retries = 0` for a plain
//! single-attempt rename).

use std::path::Path;
use std::time::Duration;

/// Abbreviated count (`1.2M`); see [`crate::fmt::number`].
pub fn format_number(n: u64) -> String {
    crate::fmt::number(n)
}

/// Byte size in full format (`1.00 GB`); see [`crate::fmt::size_full`].
pub fn format_size(bytes: u64) -> String {
    crate::fmt::size_full(bytes)
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
    fn bytes_below_kb_boundary() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn kb_boundary_switches_to_kb() {
        assert_eq!(format_size(1024), "1.00 KB");
    }

    #[test]
    fn mb_and_gb_boundaries() {
        assert_eq!(format_size(1_048_576), "1.00 MB");
        assert_eq!(format_size(1_073_741_824), "1.00 GB");
        assert_eq!(format_size(5_368_709_120), "5.00 GB");
    }

    #[test]
    fn format_number_abbreviates() {
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1.0K");
        assert_eq!(format_number(1_234_567), "1.2M");
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
        // retries = 0 is the policy hf_cache's rename_replacing delegates
        // with: one attempt, success or the raw error.
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
}
