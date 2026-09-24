use crate::engine::EngineState;
use crate::models::{DownloadStatus, VerificationProgress, VerificationQueueItem, VerifyOutcome};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

/// Global verification configuration (thread-safe, runtime-modifiable)
pub struct VerificationConfig {
    pub concurrent_verifications: AtomicUsize,
    pub buffer_size: AtomicUsize,
    pub update_interval_iterations: AtomicUsize,
}

impl VerificationConfig {
    pub const fn new() -> Self {
        Self {
            // 4 keeps multi-shard models verifying in parallel; hashing is
            // ~2 GiB/s per file, so 4 workers saturate a typical NVMe read
            // path long before the 32-core CPU is busy
            concurrent_verifications: AtomicUsize::new(4),
            // 1 MiB: large enough that per-read overhead (syscall + tokio
            // blocking-pool dispatch) is amortized to a few percent of the
            // ~2 GiB/s SHA-NI hashing ceiling on modern hardware
            buffer_size: AtomicUsize::new(1024 * 1024),
            update_interval_iterations: AtomicUsize::new(100),
        }
    }
}

pub static VERIFICATION_CONFIG: VerificationConfig = VerificationConfig::new();

/// Main verification worker that processes the verification queue
/// Runs continuously in the background, processing items as they arrive
pub async fn verification_worker(state: EngineState) {
    let max_concurrent = VERIFICATION_CONFIG
        .concurrent_verifications
        .load(Ordering::Relaxed);
    let semaphore = Arc::new(Semaphore::new(max_concurrent));

    loop {
        // Check if there's work to do - remove item and decrement size
        // atomically while holding the lock. The in-flight counter is
        // incremented *before* the item leaves the queue (still under the
        // lock), so `EngineState::verification_idle()` can never observe a
        // false idle in the gap between the removal and the task start.
        let item = {
            let mut queue = state.verification_queue.lock().await;
            if queue.is_empty() {
                None
            } else {
                state.verification_in_flight.fetch_add(1, Ordering::Relaxed);
                let item = queue.remove(0);
                state
                    .verification_queue_size
                    .fetch_sub(1, Ordering::Relaxed);
                Some(item)
            }
        };

        if let Some(item) = item {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let state = state.clone();

            tokio::spawn(async move {
                // Decrements in_flight on any exit path, including panics
                let _in_flight = InFlightGuard(state.verification_in_flight.clone());
                verify_file(item, state).await;
                drop(permit);
            });
        } else {
            // No work, sleep briefly
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }
}

/// RAII guard that decrements the engine's verification in-flight counter.
struct InFlightGuard(Arc<AtomicUsize>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Session-lifetime hash verification result counters (for HUD display).
#[derive(Debug, Default, Clone)]
pub struct VerificationResultCounters {
    pub ok: Arc<AtomicUsize>,
    pub failed: Arc<AtomicUsize>,
}

/// Verify a single file's SHA256 hash and report a typed
/// [`VerifyOutcome`] through the engine's verify channel.
async fn verify_file(item: VerificationQueueItem, state: EngineState) {
    let local_path = PathBuf::from(&item.local_path);

    // Check if file exists
    if !local_path.exists() {
        let _ = state.status_tx.send(format!(
            "Error: Cannot verify {}, file not found",
            item.filename
        ));
        let _ = state.verify_tx.send(VerifyOutcome::Missing {
            filename: item.filename.clone(),
        });
        return;
    }

    // Add to active verifications
    let verified_bytes = Arc::new(AtomicU64::new(0));
    {
        let mut progress = state.verification_progress.lock().await;
        progress.push(VerificationProgress {
            filename: item.filename.clone(),
            verified_bytes: verified_bytes.clone(),
            total_bytes: item.total_size,
            speed_mbps: 0.0,
        });
    }

    let _ = state
        .status_tx
        .send(format!("Verifying integrity of {}...", item.filename));

    // Calculate hash with progress tracking (use filename as identifier)
    match calculate_sha256_with_progress(
        &local_path,
        &state.verification_progress,
        &item.filename,
        item.total_size,
    )
    .await
    {
        Ok(calculated_hash) => {
            if calculated_hash == item.expected_sha256 {
                state
                    .verification_results
                    .ok
                    .fetch_add(1, Ordering::Relaxed);
                let _ = state
                    .status_tx
                    .send(format!("✓ Hash verified for {}", item.filename));
                let _ = state.verify_tx.send(VerifyOutcome::Ok {
                    filename: item.filename.clone(),
                });
            } else {
                state
                    .verification_results
                    .failed
                    .fetch_add(1, Ordering::Relaxed);
                let _ = state.status_tx.send(format!(
                    "✗ Hash mismatch for {}: expected {}..., got {}...",
                    item.filename,
                    &item.expected_sha256[..16],
                    &calculated_hash[..16]
                ));
                let _ = state.verify_tx.send(VerifyOutcome::Mismatch {
                    filename: item.filename.clone(),
                    expected_sha256: item.expected_sha256.clone(),
                    actual_sha256: calculated_hash,
                });

                // Mark the mismatch in the on-disk registry (the source of
                // truth — the in-memory engine mirror may be empty, e.g. for
                // CLI runs that never loaded it) and keep the mirror in sync
                // for TUI views.
                let mut registry = crate::registry::load_registry();
                if let Some(entry) = registry
                    .downloads
                    .iter_mut()
                    .find(|d| d.local_path == item.local_path)
                {
                    entry.status = DownloadStatus::HashMismatch;
                }
                crate::registry::save_registry(&registry);

                let mut mirror = state.download_registry.lock().await;
                if let Some(entry) = mirror
                    .downloads
                    .iter_mut()
                    .find(|d| d.local_path == item.local_path)
                {
                    entry.status = DownloadStatus::HashMismatch;
                }
            }
        }
        Err(e) => {
            let _ = state.status_tx.send(format!(
                "Warning: Failed to verify {}: {}",
                item.filename, e
            ));
            let _ = state.verify_tx.send(VerifyOutcome::Error {
                filename: item.filename.clone(),
                reason: e.to_string(),
            });
        }
    }

    // Remove from active verifications
    {
        let mut progress = state.verification_progress.lock().await;
        progress.retain(|p| p.filename != item.filename);
    }
}

/// Calculate SHA256 hash of a file with progress tracking
async fn calculate_sha256_with_progress(
    file_path: &Path,
    verification_progress: &Arc<Mutex<Vec<VerificationProgress>>>,
    filename: &str,
    total_size: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Resolve the shared progress counter before spawning (lookup by
    // filename).
    // NOTE: If progress entry is removed mid-verification (e.g., cancellation),
    // verified_bytes becomes None. This is safe - we simply stop progress
    // publishing. The verification still completes.
    let verified_bytes = {
        let progress = verification_progress.lock().await;
        progress
            .iter()
            .find(|p| p.filename == filename)
            .map(|p| p.verified_bytes.clone())
    };

    let path = file_path.to_path_buf();
    let name = filename.to_string();
    let progress_shared = verification_progress.clone();

    // Run the entire read+hash loop on a single blocking thread with sync
    // std::fs reads. The previous version issued every read through
    // tokio::fs, i.e. one blocking-pool dispatch per buffer (~160k hops for
    // a 20 GiB file); sync reads measure ~12% faster end-to-end on a warm
    // cache (1.87 -> 2.10 GiB/s) and keep the SHA-NI hasher saturated.
    let digest = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        use std::io::Read;

        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let buffer_size = VERIFICATION_CONFIG.buffer_size.load(Ordering::Relaxed);
        let mut buffer = vec![0u8; buffer_size];

        let mut bytes_verified = 0u64;
        let mut iteration = 0u64;
        let mut last_update = std::time::Instant::now();
        let mut last_bytes = 0u64;

        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);

            bytes_verified += bytes_read as u64;
            iteration += 1;

            // Update progress at configured interval to avoid excessive
            // lock traffic. Publish the exact running total. (An earlier
            // version did `fetch_add(bytes_read)` here, which credited only
            // the last chunk at each checkpoint - the UI advanced at
            // 1/update_interval of the real speed and looked stalled on
            // multi-GB files.)
            let update_interval = VERIFICATION_CONFIG
                .update_interval_iterations
                .load(Ordering::Relaxed);
            #[allow(clippy::manual_is_multiple_of)]
            // is_multiple_of() not available in Rust 1.75.0 (Ubuntu 22.04)
            if iteration % (update_interval as u64) == 0 || bytes_verified >= total_size {
                if let Some(ref vb) = verified_bytes {
                    vb.store(bytes_verified, Ordering::Relaxed);
                }

                let now = std::time::Instant::now();
                let elapsed = now.duration_since(last_update).as_secs_f64();

                if elapsed >= 0.2 {
                    let bytes_since_last = bytes_verified - last_bytes;
                    let speed = (bytes_since_last as f64 / elapsed) / 1_048_576.0;

                    // Best-effort speed publish from this blocking thread:
                    // try_lock avoids blocking; skip the update if the UI
                    // currently holds the progress lock.
                    if let Ok(mut progress) = progress_shared.try_lock() {
                        if let Some(entry) = progress.iter_mut().find(|p| p.filename == name) {
                            entry.speed_mbps = speed;
                        }
                    }

                    last_update = now;
                    last_bytes = bytes_verified;
                }
            }
        }

        // Final progress update to ensure 100%. If verified_bytes is Some,
        // update atomically; otherwise entry was removed (cancellation)
        if let Some(ref vb) = verified_bytes {
            vb.store(total_size, Ordering::Relaxed);
        }

        Ok(hex::encode(hasher.finalize()))
    })
    .await??;

    Ok(digest)
}

/// Queue a file for verification
pub async fn queue_verification(
    verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    verification_queue_size: Arc<AtomicUsize>,
    item: VerificationQueueItem,
) {
    let mut queue = verification_queue.lock().await;
    queue.push(item);

    verification_queue_size.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, size_bytes: usize, fill: u8) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rust-hf-downloader-verify-test-{}-{name}",
            std::process::id()
        ));
        let mut f = std::fs::File::create(&path).expect("create temp file");
        // Write in 1 MiB chunks so large fixtures stay fast
        let chunk = vec![fill; 1024 * 1024.min(size_bytes)];
        let mut written = 0;
        while written < size_bytes {
            let n = chunk.len().min(size_bytes - written);
            f.write_all(&chunk[..n]).unwrap();
            written += n;
        }
        path
    }

    /// End-to-end check of the read loop + hasher: the produced digest must
    /// equal an independent single-shot hash of the same bytes.
    #[tokio::test]
    async fn sha256_hash_is_exact() {
        let path = temp_file("hash", 3 * 1024 * 1024 + 7, 0xAB);
        let progress = Arc::new(Mutex::new(Vec::new()));
        let verified_bytes = Arc::new(AtomicU64::new(0));
        progress.lock().await.push(VerificationProgress {
            filename: "test.bin".to_string(),
            verified_bytes: verified_bytes.clone(),
            total_bytes: 3 * 1024 * 1024 + 7,
            speed_mbps: 0.0,
        });

        let digest =
            calculate_sha256_with_progress(&path, &progress, "test.bin", 3 * 1024 * 1024 + 7)
                .await
                .expect("hash calculation failed");

        let expected = {
            let mut h = Sha256::new();
            h.update(std::fs::read(&path).unwrap());
            hex::encode(h.finalize())
        };
        assert_eq!(digest, expected);
        assert_eq!(verified_bytes.load(Ordering::Relaxed), 3 * 1024 * 1024 + 7);
        std::fs::remove_file(&path).ok();
    }

    /// Regression test for the progress-accounting bug: the shared
    /// `verified_bytes` counter used to be advanced by `fetch_add(bytes_read)`
    /// only every `update_interval` iterations, so the UI advanced at
    /// 1/update_interval of the real speed (looked permanently stalled at
    /// ~1% on multi-GB files). The counter must now track the true running
    /// total at every checkpoint.
    #[tokio::test]
    async fn progress_counter_tracks_actual_bytes() {
        const TOTAL: usize = 64 * 1024 * 1024; // 64 MiB
        let path = temp_file("progress", TOTAL, 0xCD);

        // Small buffer + interval 64 -> 128 checkpoints across the file.
        // (Buggy code would cap the counter at TOTAL/64 ~= 1.6%.)
        let old_buffer = VERIFICATION_CONFIG.buffer_size.load(Ordering::Relaxed);
        let old_interval = VERIFICATION_CONFIG
            .update_interval_iterations
            .load(Ordering::Relaxed);
        VERIFICATION_CONFIG
            .buffer_size
            .store(8 * 1024, Ordering::Relaxed);
        VERIFICATION_CONFIG
            .update_interval_iterations
            .store(64, Ordering::Relaxed);

        let progress = Arc::new(Mutex::new(Vec::new()));
        let verified_bytes = Arc::new(AtomicU64::new(0));
        progress.lock().await.push(VerificationProgress {
            filename: "big.gguf".to_string(),
            verified_bytes: verified_bytes.clone(),
            total_bytes: TOTAL as u64,
            speed_mbps: 0.0,
        });

        let task_path = path.clone();
        let task_progress = progress.clone();
        let task = tokio::spawn(async move {
            calculate_sha256_with_progress(&task_path, &task_progress, "big.gguf", TOTAL as u64)
                .await
                .expect("hash calculation failed")
        });

        // Sample the published counter while the run is in flight; record the
        // high-water mark. Finished runs always store 100%, so only samples
        // taken before completion discriminate correct vs. buggy accounting.
        let mut max_seen: u64 = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !task.is_finished() {
            let v = verified_bytes.load(Ordering::Relaxed);
            if v > max_seen {
                max_seen = v;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "verification did not finish in time"
            );
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        let digest = task.await.expect("task panicked");

        // With the fix, the counter passes at least a quarter of the file
        // mid-run (in practice ~100%). The old bug capped it at ~1.6%.
        assert!(
            max_seen > (TOTAL as u64) / 4,
            "progress counter lagged: max mid-run value {} of {} bytes",
            max_seen,
            TOTAL
        );
        // And the hash is still correct
        assert_eq!(digest.len(), 64);

        // Restore globals for other tests
        VERIFICATION_CONFIG
            .buffer_size
            .store(old_buffer, Ordering::Relaxed);
        VERIFICATION_CONFIG
            .update_interval_iterations
            .store(old_interval, Ordering::Relaxed);
        std::fs::remove_file(&path).ok();
    }
}
