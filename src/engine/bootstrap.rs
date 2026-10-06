//! Engine bootstrap: the CLI startup sequence (fresh state → mirror seed →
//! both worker spawns) and the shared registry-mirror seed step the TUI's
//! startup scan reuses.

use super::workers::{spawn_manager, spawn_verification_worker, ManagerHandle};
use super::{EngineState, QueuedDownload};
use crate::models::DownloadRegistry;
use tokio::sync::mpsc;

/// Load the on-disk registry into the engine's `download_registry` mirror
/// and return the loaded snapshot.
///
/// This is the single home of the startup mirror-seed step both frontends
/// used to inline (the CLI bootstrap sites and the TUI's startup scan):
/// without it, registry updates written by the engine cannot find their
/// entries. The snapshot is returned so callers that also derive views from
/// it (the TUI's incomplete/complete lists) read the registry from disk
/// exactly once and see one consistent view.
pub async fn seed_registry_mirror(state: &EngineState) -> DownloadRegistry {
    let registry = crate::registry::load_registry();
    *state.download_registry.lock().await = registry.clone();
    registry
}

/// Bootstrap the shared engine the way the CLI frontends do:
/// [`EngineState::new`] → [`seed_registry_mirror`] →
/// [`spawn_verification_worker`] → [`spawn_manager`], in exactly that order.
///
/// `download_tx` is the sender half of the download channel: one-shot
/// callers (the CLI) send their queue and then drop it — closing the channel
/// is the deterministic completion signal (the manager's join handle
/// resolves). The verification worker's join handle is detached, exactly
/// like the inline bootstrap this replaces.
///
/// The TUI cannot use this function — its `App::new` is sync and must build
/// the engine before any await point — so it composes the same pieces:
/// [`EngineState::new`] in `App::new`, [`seed_registry_mirror`] in its
/// startup scan, the two spawns in `App::run`.
pub async fn bootstrap() -> (
    EngineState,
    mpsc::UnboundedSender<QueuedDownload>,
    ManagerHandle,
) {
    let (state, download_tx) = EngineState::new();
    seed_registry_mirror(&state).await;
    spawn_verification_worker(state.clone());
    let manager = spawn_manager(state.clone());
    (state, download_tx, manager)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{closed_port, EnvGuard};

    // Env-mutating tests share the crate-wide mutex from `paths` (see the
    // note in `enqueue`'s tests): one mutex serializes every test that
    // touches the process-global registry location.
    use crate::models::{DownloadMetadata, DownloadStatus};
    use crate::paths::ENV_MUTEX;

    /// One registry entry to write to disk, exercising the same field set
    /// `registry::register_pending` produces.
    fn sample_registry() -> DownloadRegistry {
        DownloadRegistry {
            downloads: vec![DownloadMetadata {
                model_id: "a/b".to_string(),
                filename: "f.bin".to_string(),
                url: "https://example.invalid/a/b/resolve/main/f.bin".to_string(),
                local_path: "/tmp/f.bin".to_string(),
                total_size: 10,
                downloaded_size: 0,
                status: DownloadStatus::Incomplete,
                expected_sha256: None,
                revision: None,
            }],
        }
    }

    #[tokio::test]
    // Holding the (std) env mutex across the awaits below is intentional:
    // it serializes env-mutating tests; other tokio workers keep making
    // progress.
    #[allow(clippy::await_holding_lock)]
    async fn seed_registry_mirror_loads_disk_into_mirror_and_returns_snapshot() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-seed-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, _tx) = EngineState::new();

        // No registry file on disk → empty mirror, empty snapshot (the
        // pre-seed mirror state is also empty, so this pins the default).
        let snapshot = seed_registry_mirror(&state).await;
        assert!(snapshot.downloads.is_empty());
        assert!(state.download_registry.lock().await.downloads.is_empty());

        // Registry file with one entry → both the mirror and the returned
        // snapshot match what is on disk (whole-registry assign, the
        // semantics both CLI bootstrap sites and the TUI scan used inline).
        crate::registry::save_registry(&sample_registry());
        let snapshot = seed_registry_mirror(&state).await;
        assert_eq!(snapshot.downloads.len(), 1);
        assert_eq!(snapshot.downloads[0].filename, "f.bin");
        let mirror = state.download_registry.lock().await;
        assert_eq!(mirror.downloads.len(), 1);
        assert_eq!(mirror.downloads[0].filename, snapshot.downloads[0].filename);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn bootstrap_seeds_mirror_and_spawns_draining_manager() {
        let _env_lock = ENV_MUTEX.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("engine-bootstrap-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));
        crate::registry::save_registry(&sample_registry());

        let (state, download_tx, manager) = bootstrap().await;

        // Mirror seeded from disk as part of bootstrap, before the workers
        // were handed the state (the seeded mirror is what verification
        // updates look their entries up in).
        assert_eq!(state.download_registry.lock().await.downloads.len(), 1);

        // The manager was spawned: closing the channel makes its join
        // handle resolve (zero files queued → empty outcome list).
        drop(download_tx);
        let outcomes = manager.join.await.expect("manager task panicked");
        assert!(outcomes.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
