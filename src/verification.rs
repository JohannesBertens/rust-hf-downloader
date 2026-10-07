//! SHA256 verification worker: drains the verification queue,
//! hashes files (bounded by a semaphore), reports typed
//! [`VerifyOutcome`]s over the engine's outcome channel, and signals
//! idle when the queue is exhausted.
//!
//! # The hub seam (M3)
//!
//! [`VerificationHub`] is the verification worker's ENTIRE shared state:
//! the queue it drains, the counters that make [`VerificationHub::idle`]
//! race-free, the progress list the UI snapshots, the session result
//! counters, the two channels it reports through, and the registry mirror
//! it patches on mismatch. The engine constructs the hub from
//! `EngineState` (in `engine/workers.rs`) and hands it to
//! [`verification_worker`]; this module therefore imports models +
//! registry ONLY — never the engine module — which turns the old
//! engine↔verification cycle into a DAG edge (engine → verification).
//! The textual side of that invariant is pinned by
//! `tests/docs_guards.rs::module_dependency_dag`.

use crate::models::{DownloadRegistry, VerificationProgress, VerificationQueueItem, VerifyOutcome};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, Semaphore};

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

/// The verification worker's entire shared state (M3): the 8 handles the
/// worker consumes, grouped so the worker signature is
/// `verification_worker(hub)` and `verify_file(item, &hub)` — no
/// `EngineState` in this module. Constructed by the engine
/// (`engine/workers.rs`) from the engine's own channels and Arcs; every
/// field is an Arc or a channel endpoint, so cloning the hub is cheap and
/// shares the same underlying state.
///
/// `registry_mirror` is the SAME `Arc<Mutex<DownloadRegistry>>` the engine
/// holds as its registry mirror — one mutex, shared into the hub (see the
/// field→bundle table in `engine/mod.rs`).
#[derive(Clone, Debug)]
pub struct VerificationHub {
    /// Pending work: items waiting to be hashed.
    pub queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    /// Number of items in `queue` (lock-free read for HUD/`idle`).
    pub size: Arc<AtomicUsize>,
    /// Number of verification tasks spawned but not yet finished.
    /// Incremented while the queue lock is held (before an item is
    /// removed), so `idle()` can never observe a false idle between the
    /// queue removal and the task start.
    pub in_flight: Arc<AtomicUsize>,
    /// Active verifications (the list the UI snapshots for its HUD rows).
    pub progress: Arc<Mutex<Vec<VerificationProgress>>>,
    /// Session-lifetime ok/failed counters (HUD footer / CLI summary).
    pub results: VerificationResultCounters,
    /// Free-text status channel (the engine's `EventBus` status sender).
    pub status_tx: mpsc::UnboundedSender<String>,
    /// Typed verification results (the engine's `EventBus` verify sender).
    pub verify_tx: mpsc::UnboundedSender<VerifyOutcome>,
    /// The engine's in-memory registry mirror (shared Arc), patched on
    /// hash mismatch after the on-disk op.
    pub registry_mirror: Arc<Mutex<DownloadRegistry>>,
}

impl VerificationHub {
    /// True when no verification work is queued or running (the two
    /// lock-free counters read together; race-free because `in_flight` is
    /// incremented under the queue lock before an item leaves `queue`).
    pub fn idle(&self) -> bool {
        self.size.load(Ordering::Relaxed) == 0 && self.in_flight.load(Ordering::Relaxed) == 0
    }

    /// Queue a file for verification: push under the queue lock, then
    /// bump the size counter — exactly the two steps the former free
    /// function performed (same lock scope, same ordering).
    pub async fn queue_verification(&self, item: VerificationQueueItem) {
        let mut queue = self.queue.lock().await;
        queue.push(item);

        self.size.fetch_add(1, Ordering::Relaxed);
    }
}

/// Main verification worker that processes the verification queue
/// Runs continuously in the background, processing items as they arrive
pub async fn verification_worker(hub: VerificationHub) {
    let max_concurrent = VERIFICATION_CONFIG
        .concurrent_verifications
        .load(Ordering::Relaxed);
    let semaphore = Arc::new(Semaphore::new(max_concurrent));

    loop {
        // Check if there's work to do - remove item and decrement size
        // atomically while holding the lock. The in-flight counter is
        // incremented *before* the item leaves the queue (still under the
        // lock), so `VerificationHub::idle()` can never observe a false
        // idle in the gap between the removal and the task start.
        let item = {
            let mut queue = hub.queue.lock().await;
            if queue.is_empty() {
                None
            } else {
                hub.in_flight.fetch_add(1, Ordering::Relaxed);
                let item = queue.remove(0);
                hub.size.fetch_sub(1, Ordering::Relaxed);
                Some(item)
            }
        };

        if let Some(item) = item {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let hub = hub.clone();

            tokio::spawn(async move {
                // Decrements in_flight on any exit path, including panics
                let _in_flight = InFlightGuard(hub.in_flight.clone());
                verify_file(item, &hub).await;
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
/// [`VerifyOutcome`] through the hub's verify channel.
async fn verify_file(item: VerificationQueueItem, hub: &VerificationHub) {
    let local_path = PathBuf::from(&item.local_path);

    // Check if file exists
    if !local_path.exists() {
        let _ = hub.status_tx.send(format!(
            "Error: Cannot verify {}, file not found",
            item.filename
        ));
        let _ = hub.verify_tx.send(VerifyOutcome::Missing {
            filename: item.filename.clone(),
        });
        return;
    }

    // Add to active verifications
    let verified_bytes = Arc::new(AtomicU64::new(0));
    {
        let mut progress = hub.progress.lock().await;
        progress.push(VerificationProgress {
            filename: item.filename.clone(),
            verified_bytes: verified_bytes.clone(),
            total_bytes: item.total_size,
            speed_mbps: 0.0,
        });
    }

    let _ = hub
        .status_tx
        .send(format!("Verifying integrity of {}...", item.filename));

    // Calculate hash with progress tracking (use filename as identifier)
    match calculate_sha256_with_progress(
        &local_path,
        &hub.progress,
        &item.filename,
        item.total_size,
    )
    .await
    {
        Ok(calculated_hash) => {
            if calculated_hash == item.expected_sha256 {
                hub.results.ok.fetch_add(1, Ordering::Relaxed);
                let _ = hub
                    .status_tx
                    .send(format!("✓ Hash verified for {}", item.filename));
                let _ = hub.verify_tx.send(VerifyOutcome::Ok {
                    filename: item.filename.clone(),
                });
            } else {
                hub.results.failed.fetch_add(1, Ordering::Relaxed);
                let expected_chars: Vec<char> = item.expected_sha256.chars().collect();
                let expected_trunc: String = expected_chars.iter().take(16).collect();
                let expected_ell = if expected_chars.len() > 16 { "..." } else { "" };
                let _ = hub.status_tx.send(format!(
                    "✗ Hash mismatch for {}: expected {}{}, got {}...",
                    item.filename,
                    expected_trunc,
                    expected_ell,
                    &calculated_hash[..16]
                ));
                let _ = hub.verify_tx.send(VerifyOutcome::Mismatch {
                    filename: item.filename.clone(),
                    expected_sha256: item.expected_sha256.clone(),
                    actual_sha256: calculated_hash,
                });

                // Mark the mismatch in the on-disk registry (the source of
                // truth — the in-memory engine mirror may be empty, e.g. for
                // CLI runs that never loaded it), then patch the mirror for
                // TUI views. Layering (final pass): the disk op is the pure
                // registry op; the engine-mirror patch lives here, with the
                // caller — same timing as when the op did both: mirror
                // immediately after the disk save, regardless of its
                // outcome.
                crate::registry::mark_mismatch(&local_path);
                mark_mismatch_mirror(&hub.registry_mirror, &local_path).await;
            }
        }
        Err(e) => {
            let _ = hub.status_tx.send(format!(
                "Warning: Failed to verify {}: {}",
                item.filename, e
            ));
            let _ = hub.verify_tx.send(VerifyOutcome::Error {
                filename: item.filename.clone(),
                reason: e.to_string(),
            });
        }
    }

    // Remove from active verifications
    {
        let mut progress = hub.progress.lock().await;
        progress.retain(|p| p.filename != item.filename);
    }
}

/// Patch the engine's in-memory registry mirror for a SHA mismatch
/// (final layering pass: moved out of the `registry::mark_mismatch` op so
/// that module is pure disk ops; the engine mirror is engine-caller
/// state). Same contract as when the op owned it: called immediately
/// after the disk op, regardless of whether the disk save succeeded; the
/// mirror is patched independently and may lack the entry entirely (the
/// disk is the source of truth). Pinned by the tests below.
async fn mark_mismatch_mirror(
    download_registry: &Arc<Mutex<crate::models::DownloadRegistry>>,
    local_path: &Path,
) {
    let mut mirror = download_registry.lock().await;
    if let Some(entry) = mirror
        .downloads
        .iter_mut()
        .find(|d| crate::registry::path_matches(&d.local_path, local_path))
    {
        entry.status = crate::models::DownloadStatus::HashMismatch;
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
    // The loop itself is the shared streaming-digest core
    // (`utils::stream_file_digest`); this closure only contributes the
    // progress accounting.
    let digest = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        let buffer_size = VERIFICATION_CONFIG.buffer_size.load(Ordering::Relaxed);
        let mut hasher = Sha256::new();
        let mut iteration = 0u64;
        let mut last_update = std::time::Instant::now();
        let mut last_bytes = 0u64;

        let _ = crate::utils::stream_file_digest(
            &path,
            &mut hasher,
            buffer_size,
            |_, bytes_verified| {
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
            },
        )?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A standalone hub with its own channels (the engine wiring — shared
    /// channels, shared registry Arc — is pinned by the engine-side shape
    /// test; these tests only need the worker's own behavior).
    fn fresh_hub() -> (VerificationHub, mpsc::UnboundedReceiver<String>) {
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        let (verify_tx, _verify_rx) = mpsc::unbounded_channel();
        (
            VerificationHub {
                queue: Arc::new(Mutex::new(Vec::new())),
                size: Arc::new(AtomicUsize::new(0)),
                in_flight: Arc::new(AtomicUsize::new(0)),
                progress: Arc::new(Mutex::new(Vec::new())),
                results: VerificationResultCounters::default(),
                status_tx,
                verify_tx,
                registry_mirror: Arc::new(Mutex::new(crate::models::DownloadRegistry::default())),
            },
            status_rx,
        )
    }

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
    // Holding the (std) env mutex across the awaits below is intentional:
    // this test sets non-default VERIFICATION_CONFIG values and samples
    // progress mid-flight, so it must be serialized against any test that
    // runs `config::apply_options` (which writes the same globals) — the
    // T1/W-final matrix and options-dialog tests all take this mutex.
    #[allow(clippy::await_holding_lock)]
    async fn progress_counter_tracks_actual_bytes() {
        let _env_lock = crate::paths::ENV_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

    // ----------------- mark_mismatch_mirror (final layering pass) ---------
    // The mirror patch moved out of `registry::mark_mismatch` so that
    // module is pure disk ops; these tests pin its caller-side contract:
    // only the matching entry flips, an empty mirror stays empty, and a
    // mirror lacking the entry is untouched. In-memory only — no registry
    // path involved, so no ENV_MUTEX/DataDirGuard is needed.

    fn mirror_with(
        entries: &[(&str, crate::models::DownloadStatus)],
    ) -> Arc<Mutex<crate::models::DownloadRegistry>> {
        Arc::new(Mutex::new(crate::models::DownloadRegistry {
            downloads: entries
                .iter()
                .map(|(path, status)| crate::models::DownloadMetadata {
                    model_id: "org/model".to_string(),
                    filename: path.rsplit('/').next().unwrap().to_string(),
                    url: format!("https://huggingface.co/org/model/resolve/main/{}", path),
                    local_path: path.to_string(),
                    total_size: 1,
                    downloaded_size: 0,
                    status: status.clone(),
                    expected_sha256: None,
                    revision: None,
                })
                .collect(),
        }))
    }

    #[tokio::test]
    async fn mismatch_mirror_patch_flips_only_the_matching_entry() {
        let mirror = mirror_with(&[
            ("/x/a.gguf", crate::models::DownloadStatus::Complete),
            ("/x/b.gguf", crate::models::DownloadStatus::Incomplete),
        ]);
        mark_mismatch_mirror(&mirror, std::path::Path::new("/x/a.gguf")).await;
        let m = mirror.lock().await;
        assert_eq!(
            m.downloads[0].status,
            crate::models::DownloadStatus::HashMismatch
        );
        assert_eq!(
            m.downloads[1].status,
            crate::models::DownloadStatus::Incomplete
        );
    }

    #[tokio::test]
    async fn mismatch_mirror_patch_leaves_empty_and_unmatched_mirrors_untouched() {
        // The CLI-before-bootstrap case: an empty mirror stays empty (the
        // disk is the source of truth).
        let empty = mirror_with(&[]);
        mark_mismatch_mirror(&empty, std::path::Path::new("/x/missing.gguf")).await;
        assert!(empty.lock().await.downloads.is_empty());

        // A mirror lacking the matching entry is untouched.
        let other = mirror_with(&[("/x/b.gguf", crate::models::DownloadStatus::Incomplete)]);
        mark_mismatch_mirror(&other, std::path::Path::new("/x/a.gguf")).await;
        assert_eq!(
            other.lock().await.downloads[0].status,
            crate::models::DownloadStatus::Incomplete
        );
    }

    #[tokio::test]
    async fn short_expected_sha256_does_not_panic_on_mismatch() {
        let tmp = std::env::temp_dir().join(format!("test-b2-short-sha-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let file_path = tmp.join("test.bin");
        std::fs::write(&file_path, b"test payload").unwrap();

        let (hub, status_rx) = fresh_hub();
        let item = VerificationQueueItem {
            filename: "test.bin".to_string(),
            local_path: file_path.to_string_lossy().to_string(),
            expected_sha256: "short".to_string(),
            total_size: 12,
            is_manual: true,
        };

        verify_file(item, &hub).await;

        assert_eq!(hub.results.failed.load(Ordering::Relaxed), 1);
        let status_msg = {
            let mut rx = status_rx;
            let mut msgs = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                msgs.push(msg);
            }
            msgs
        };
        assert!(
            status_msg.iter().any(|m| m.contains("expected short, got")),
            "status should contain char-safe fallback without a misleading \
             ellipsis on a non-truncated short hash: {:?}",
            status_msg
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn long_expected_sha256_still_truncates_with_ellipsis() {
        // Pins the OTHER branch of the conditional ellipsis: a full
        // 64-char expected hash is truncated to 16 chars and keeps "...".
        let tmp = std::env::temp_dir().join(format!("test-b2-long-sha-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let file_path = tmp.join("test.bin");
        std::fs::write(&file_path, b"test payload").unwrap();

        let (hub, status_rx) = fresh_hub();
        let expected = "a".repeat(64);
        let item = VerificationQueueItem {
            filename: "test.bin".to_string(),
            local_path: file_path.to_string_lossy().to_string(),
            expected_sha256: expected.clone(),
            total_size: 12,
            is_manual: true,
        };

        verify_file(item, &hub).await;

        let status_msg = {
            let mut rx = status_rx;
            let mut msgs = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                msgs.push(msg);
            }
            msgs
        };
        let truncated = format!("expected {}..., got", "a".repeat(16));
        assert!(
            status_msg.iter().any(|m| m.contains(&truncated)),
            "status should truncate a 64-char expected hash to 16 chars with \
             an ellipsis (contained {:?}): {:?}",
            truncated,
            status_msg
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
