//! The engine's worker tasks: the download manager (channel consumer) and
//! the verification worker bootstrap.

use super::{EngineState, QueuedDownload};
use crate::download::{start_download, DownloadParams};
use crate::models::FileOutcome;
use tokio::task::JoinHandle;

/// Handle to the running download manager.
pub struct ManagerHandle {
    /// Resolves with one [`FileOutcome`] per processed file once the download
    /// channel is closed and fully drained. The TUI simply drops this handle
    /// (the task keeps running); the CLI awaits it for completion.
    pub join: JoinHandle<Vec<FileOutcome>>,
}

/// Spawn the download manager task.
///
/// Consumes the `download_rx` channel serially (one file at a time;
/// parallelism is within a file's chunks, provided by
/// [`crate::download::start_download`]) and maintains the queue accounting
/// the TUI HUD renders from.
pub fn spawn_manager(state: EngineState) -> ManagerHandle {
    let join = tokio::spawn(async move {
        let mut outcomes = Vec::new();

        loop {
            // Lock only when receiving, release immediately after. This
            // prevents deadlock by not holding download_rx while acquiring
            // other locks (see AGENTS.md lock hierarchy).
            let download = {
                let mut rx = state.download_rx.lock().await;
                match rx.recv().await {
                    Some(msg) => msg,
                    None => break, // Channel closed and drained
                }
            };
            let QueuedDownload {
                model_id,
                revision,
                filename,
                base_path,
                expected_sha256,
                hf_token,
                total_size,
            } = download;

            // Decrement the totals and drop the mirrored queue item (first
            // match by filename) — one method; the two locks are acquired
            // separately, never nested (AGENTS.md per-bundle invariants).
            state.queue.remove_started(&filename, total_size).await;

            let outcome = start_download(DownloadParams {
                model_id,
                revision,
                filename,
                base_path,
                progress: state.download_progress.clone(),
                status_tx: state.events.status_tx.clone(),
                complete_downloads: state.complete_downloads.clone(),
                expected_sha256,
                verification: state.verification.clone(),
                hf_token,
            })
            .await;

            // Stream per-file outcomes to live consumers (the join handle
            // still returns the complete list for drain-based callers).
            let _ = state.events.outcome_tx.send(outcome.clone());

            outcomes.push(outcome);
        }

        outcomes
    });

    ManagerHandle { join }
}

/// Spawn the background verification worker (runs until the process exits).
pub fn spawn_verification_worker(state: EngineState) -> JoinHandle<()> {
    tokio::spawn(crate::verification::verification_worker(state.verification))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{closed_port, EnvGuard};

    // Env-mutating tests share the crate-wide mutex from `paths` (see the
    // note in `enqueue`'s tests): one mutex serializes every test that
    // touches the process-global registry location.
    use crate::paths::ENV_MUTEX;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    #[tokio::test]
    // Holding the (std) env mutex across the await below is intentional: it
    // serializes env-mutating tests; other tokio workers keep making progress.
    #[allow(clippy::await_holding_lock)]
    async fn manager_drains_when_channel_closed() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("engine-drain-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        // Fail fast: no retries
        let old_retries = crate::download::DOWNLOAD_CONFIG
            .max_retries
            .load(Ordering::Relaxed);
        crate::download::DOWNLOAD_CONFIG
            .max_retries
            .store(0, Ordering::Relaxed);

        let (state, tx) = EngineState::new();
        let handle = spawn_manager(state.clone());

        for name in ["f.bin", "g.bin"] {
            tx.send(QueuedDownload {
                model_id: "a/b".to_string(),
                revision: crate::api::DEFAULT_REVISION.to_string(),
                filename: name.to_string(),
                base_path: tmp.clone(),
                expected_sha256: None,
                hf_token: None,
                total_size: 10,
            })
            .unwrap();
        }
        drop(tx); // closes the channel → manager drains and resolves

        let outcomes = handle.join.await.expect("manager task panicked");
        assert_eq!(outcomes.len(), 2, "one outcome per queued file");
        assert!(
            matches!(&outcomes[0], FileOutcome::Failed { filename, .. } if filename == "f.bin")
        );
        assert!(
            matches!(&outcomes[1], FileOutcome::Failed { filename, .. } if filename == "g.bin")
        );

        // Queue accounting drained back to zero
        assert_eq!(state.queue.download_queue_totals.lock().await.size, 0);

        crate::download::DOWNLOAD_CONFIG
            .max_retries
            .store(old_retries, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn queued_download_fields_roundtrip_through_the_channel() {
        // QueuedDownload is a named struct, so the two adjacent
        // Option<String> fields (expected_sha256 / hf_token) can no longer
        // be swapped at a construction site without the compiler catching
        // it. Pin the field meaning by roundtripping one message through the
        // engine channel and reading every field back by name.
        let (state, tx) = EngineState::new();
        tx.send(QueuedDownload {
            model_id: "author/model".to_string(),
            revision: "deadbeef".to_string(),
            filename: "sub/dir/file.bin".to_string(),
            base_path: PathBuf::from("/tmp/base/author/model"),
            expected_sha256: Some("abc123".to_string()),
            hf_token: None,
            total_size: 42,
        })
        .unwrap();
        drop(tx);

        let received = { state.download_rx.lock().await.recv().await }.expect("message queued");
        assert_eq!(received.model_id, "author/model");
        assert_eq!(received.revision, "deadbeef");
        assert_eq!(received.filename, "sub/dir/file.bin");
        assert_eq!(received.base_path, PathBuf::from("/tmp/base/author/model"));
        assert_eq!(received.expected_sha256.as_deref(), Some("abc123"));
        assert_eq!(received.hf_token, None);
        assert_eq!(received.total_size, 42);
    }
}
