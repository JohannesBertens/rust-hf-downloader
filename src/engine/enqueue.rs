//! The enqueue transaction (W2.1) and its sealed policy types.
//!
//! [`EngineState::enqueue`] is the single home of the enqueue sequence
//! (registry bookkeeping per policy → queue accounting → HUD mirror →
//! channel sends → failed-send rollback) that four TUI flows and two CLI
//! flows used to inline; every per-site divergence is an explicit,
//! constructor-sealed [`EnqueuePolicy`] knob, and status/error strings stay
//! at the call sites (see [`EnqueueOutcome`]). The 8 characterization tests
//! at the bottom pin each flavor byte-for-byte.

use super::{EngineState, QueuedDownload};
use crate::models::{DownloadMetadata, DownloadStatus, QueueItemSummary};
use tokio::sync::mpsc;

impl EngineState {
    /// The enqueue transaction (W2.1): registry bookkeeping per policy →
    /// `download_queue.add` → `download_queue_items` push + `download_tx`
    /// sends → failed-send rollback. One home for the six sequences that
    /// used to be inline (four TUI flows + two CLI flows); every divergence
    /// between them is an explicit [`EnqueuePolicy`] knob, and status/error
    /// strings stay at the call sites (see [`EnqueueOutcome`]).
    ///
    /// Lock order (AGENTS.md hierarchy): every lock is acquired in its own
    /// scope, never nested — mirror/queue/items acquisitions only ever
    /// happen sequentially, exactly like the inline code this replaces.
    ///
    /// The sender is passed in, not stored on `EngineState`: holding a
    /// sender clone would keep the channel open forever and break the CLI's
    /// drop-based deterministic completion (dropping *every* sender is the
    /// drain signal).
    pub async fn enqueue(
        &self,
        tx: &mpsc::UnboundedSender<QueuedDownload>,
        files: &[QueuedDownload],
        policy: &EnqueuePolicy,
    ) -> EnqueueOutcome {
        let total_bytes: u64 = files.iter().map(|f| f.total_size).sum();

        // --- 1. Registry bookkeeping, before any queue work (the order of
        //         every legacy site) ---------------------------------------
        let (invalid, aborted) = match &policy.registry {
            // The two no-write flavors share one arm (`on_invalid` =
            // SkipValidation): neither validates paths at enqueue time
            // — resume re-queues entries that were validated when first
            // recorded, hf-cache mirrors the local cache layout, and
            // `start_download` sanitizes every filename component before
            // writing regardless.
            RegistryMode::AlreadyRecorded | RegistryMode::StagingSweep => (Vec::new(), None),
            RegistryMode::Mirror { base, entry_size } => {
                // TUI confirm flows: mirror clone → per-file validate (an
                // invalid file is skipped from the registry and reported,
                // but still queued) → append entries whose url is not
                // recorded yet → save the whole registry → re-assign the
                // mirror. Deliberately writes from the MIRROR, not from
                // disk — today's TUI behavior, kept byte-for-byte (the
                // disk-source-of-truth ops in `registry.rs` are the
                // engine/CLI flavor).
                let mut registry = self.download_registry.lock().await.clone();
                let mut skipped = Vec::new();
                for file in files {
                    let validated = match crate::paths::sanitize::validate_and_sanitize_path(
                        base,
                        &file.model_id,
                        &file.filename,
                    ) {
                        Ok(path) => Some(path),
                        Err(e) => {
                            // on_invalid = ReportAndQueue: report the
                            // file and keep it out of the registry —
                            // the queue work below still queues it.
                            // (No constructor pairs a mirror write
                            // with the other flavors; if one ever
                            // does, its invalid files stay out of the
                            // registry too.)
                            if policy.on_invalid == InvalidPolicy::ReportAndQueue {
                                skipped.push((file.filename.clone(), e));
                            }
                            None
                        }
                    };
                    if let Some(validated) = validated {
                        let url =
                            crate::api::resolve_url(&file.model_id, &file.filename, &file.revision);
                        if !registry.downloads.iter().any(|d| d.url == url) {
                            registry.downloads.push(DownloadMetadata {
                                model_id: file.model_id.clone(),
                                filename: file.filename.clone(),
                                url,
                                local_path: validated.to_string_lossy().to_string(),
                                total_size: match entry_size {
                                    RegistryEntrySize::Zero => 0,
                                    RegistryEntrySize::FromQueued => file.total_size,
                                },
                                downloaded_size: 0,
                                status: DownloadStatus::Incomplete,
                                expected_sha256: file.expected_sha256.clone(),
                                // One rule reproduces both legacy flavors: the
                                // TUI always queues the default revision (→
                                // `None`, as its inline code hardcoded) and the
                                // CLI records the revision only when it differs
                                // from the default.
                                revision: if file.revision == crate::api::DEFAULT_REVISION {
                                    None
                                } else {
                                    Some(file.revision.clone())
                                },
                            });
                        }
                    }
                }
                crate::registry::save_registry(&registry);
                *self.download_registry.lock().await = registry;
                (skipped, None)
            }
            RegistryMode::Disk { base } => {
                // CLI download flow (on_invalid = AbortAll): validate
                // every file first — the first invalid filename aborts
                // the whole enqueue (nothing is queued or sent) — then
                // upsert the entries on DISK. This is exactly
                // `registry::register_pending`, whose byte-level behavior the
                // registry golden tests pin. A single-model batch is
                // assumed (the CLI flavor's shape), so the first file's
                // model id and revision stand for the whole batch, exactly
                // like the one-`model_id`-one-`revision` call it replaces.
                let model_id = files
                    .first()
                    .map(|f| f.model_id.clone())
                    .unwrap_or_default();
                let revision = files
                    .first()
                    .map(|f| f.revision.clone())
                    .unwrap_or_else(|| crate::api::DEFAULT_REVISION.to_string());
                let pending: Vec<(String, u64, Option<String>)> = files
                    .iter()
                    .map(|f| (f.filename.clone(), f.total_size, f.expected_sha256.clone()))
                    .collect();
                (
                    Vec::new(),
                    crate::registry::register_pending(&model_id, &revision, &pending, base).err(),
                )
            }
        };
        if aborted.is_some() {
            // Validate-first abort: no queue work, no sends (CLI policy).
            return EnqueueOutcome {
                sent: 0,
                invalid,
                aborted,
            };
        }

        // --- 2. Queue accounting, before the sends for every discipline
        //         but resume --------------------------------------------------
        if policy.discipline != SendDiscipline::Resume {
            self.download_queue
                .lock()
                .await
                .add(files.len(), total_bytes);
        }

        // --- 3. HUD mirror + sends, in the discipline's shape ----------------
        let mut sent = 0;
        match policy.discipline {
            SendDiscipline::Interactive => {
                // TUI confirm flavors: a summary lands only for files that
                // made it onto the channel.
                for file in files {
                    if tx.send(file.clone()).is_ok() {
                        sent += 1;
                        let mut items = self.download_queue_items.lock().await;
                        items.push(queue_item_summary(file));
                    }
                }
            }
            SendDiscipline::Resume => {
                // Resume flavor: a summary per file regardless of send result.
                for file in files {
                    if tx.send(file.clone()).is_ok() {
                        sent += 1;
                    }
                    let mut items = self.download_queue_items.lock().await;
                    items.push(queue_item_summary(file));
                }
            }
            SendDiscipline::Batch => {
                // CLI flavors: every summary first (one lock), then sends.
                {
                    let mut items = self.download_queue_items.lock().await;
                    for file in files {
                        items.push(queue_item_summary(file));
                    }
                }
                for file in files {
                    if tx.send(file.clone()).is_ok() {
                        sent += 1;
                    }
                }
            }
        }

        // --- 4. Resume discipline accounts the queue once, after the
        //         sends -------------------------------------------------------
        if policy.discipline == SendDiscipline::Resume {
            self.download_queue
                .lock()
                .await
                .add(files.len(), total_bytes);
        }

        // --- 5. Failed-send rollback (Interactive only): remove the
        //         failed tail from the queue accounting — today's
        //         arithmetic, which charges the bytes of the files after
        //         the first `sent` ones (exact while failures are
        //         tail-contiguous, as they are for a closed channel). The
        //         HUD mirror needs no rollback: Interactive only ever
        //         pushed for successful sends.
        if policy.discipline == SendDiscipline::Interactive && sent < files.len() {
            let failed_bytes: u64 = files.iter().skip(sent).map(|f| f.total_size).sum();
            self.download_queue
                .lock()
                .await
                .remove(files.len() - sent, failed_bytes);
        }

        EnqueueOutcome {
            sent,
            invalid,
            aborted: None,
        }
    }
}

/// Which registry bookkeeping the enqueue transaction performs (divergence
/// knob 1 of the W2.1 table; see [`EngineState::enqueue`]).
#[derive(Debug, Clone)]
pub enum RegistryMode {
    /// TUI resume: the entries already exist on disk (recorded when the
    /// files were first queued) — re-queueing must not touch them.
    AlreadyRecorded,
    /// `hf-cache sync`: registers NOTHING pending — the named
    /// staging-sweep decision (the download path writes staging-path
    /// entries during the run; the sweeps at publish time and next
    /// bootstrap remove them, keeping the TUI's resume view clean; see
    /// `cli/hf_cache/sync.rs`). The former `None` variant was split into
    /// these two names because it conflated both intents (and collided
    /// with `Option::None` under glob imports).
    StagingSweep,
    /// TUI confirm flows: read the engine's registry MIRROR, append an
    /// `Incomplete` entry for every file whose path validates and whose
    /// url is not recorded yet (invalid files are skipped and reported —
    /// they are still queued), save the whole registry to disk, then
    /// re-assign the mirror.
    Mirror {
        /// Raw user base directory used to validate each file and derive
        /// its `local_path` — NOT the queued `base_path`, which already
        /// includes `author/model` (or the staging dir).
        base: String,
        /// `total_size` recorded in the registry entries.
        entry_size: RegistryEntrySize,
    },
    /// CLI download flow: validate every file first — the first invalid
    /// filename aborts the whole enqueue (reported via
    /// [`EnqueueOutcome::aborted`]) — then upsert the entries on DISK
    /// (`register_pending`/`upsert_pending`, both in `registry.rs` —
    /// pending writes have one owner). The engine's
    /// mirror is not patched: the CLI seeds it from disk at bootstrap.
    Disk {
        /// Raw user base directory (same meaning as [`RegistryMode::Mirror`]'s
        /// `base`).
        base: String,
    },
}

/// `total_size` written into registry entries by [`RegistryMode::Mirror`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryEntrySize {
    /// The GGUF quant-group flow records `0` — today's behavior.
    Zero,
    /// Every other registering flow records the queued file size.
    FromQueued,
}

/// How the channel sends interact with the queue accounting and the HUD
/// mirror (divergence knob 2 of the W2.1 table; see
/// [`EngineState::enqueue`]). One enum replaces the three independently
/// combinable knobs it used to be (`QueueTiming` + `ItemsMirror` +
/// `failed_send_rollback`): the six legacy inline sites used exactly
/// three correlated combinations, so the type now makes every other mix
/// unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendDiscipline {
    /// TUI confirm flavors: the queue is accounted BEFORE the sends, a
    /// HUD summary lands only per SUCCESSFUL send, and the failed-send
    /// tail is rolled back out of the queue accounting.
    Interactive,
    /// TUI resume flavor: every send happens first, the queue is
    /// accounted once AFTER them, a HUD summary lands per file
    /// regardless of send result, and nothing is ever rolled back.
    Resume,
    /// CLI flavors: every HUD summary is pushed up front (one lock),
    /// the queue is accounted before the sends, and nothing is rolled
    /// back.
    Batch,
}

/// What the enqueue transaction does with a file whose path fails
/// validation (divergence knob 3; the rule used to live in comments
/// inside the registry arms above).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidPolicy {
    /// Mirror-registry flavors (TUI confirms): the invalid file is
    /// skipped from the registry and reported via
    /// [`EnqueueOutcome::invalid`] — but still queued and sent.
    ReportAndQueue,
    /// Disk-registry flavor (CLI `download`): the first invalid
    /// filename aborts the whole enqueue (via
    /// `registry::register_pending`'s validate-first pass) — nothing is
    /// queued or sent; the error is
    /// reported via [`EnqueueOutcome::aborted`].
    AbortAll,
    /// The no-registry flavors (TUI resume, `hf-cache sync`): no path
    /// validation runs at enqueue time today — resume re-queues entries
    /// that were validated when first recorded, hf-cache mirrors the
    /// local cache layout, and `start_download` sanitizes every
    /// filename component before writing regardless.
    SkipValidation,
}

/// The full shape of one enqueue transaction: every field is a
/// documented divergence between the six legacy inline sites (see the
/// W2.1 divergence table). The fields are private on purpose — the five
/// named constructors are the only public API (the design-review sealing
/// decision: the raw knobs were N ways to spell one of five known
/// flavors, and only their correlated combinations ever occurred).
#[derive(Debug, Clone)]
pub struct EnqueuePolicy {
    /// Registry bookkeeping: TUI mirror upsert / CLI disk upsert /
    /// no writes (resume, staging sweep).
    registry: RegistryMode,
    /// How the sends interact with queue accounting and the HUD mirror.
    discipline: SendDiscipline,
    /// What happens to a file whose path fails validation.
    on_invalid: InvalidPolicy,
}

impl EnqueuePolicy {
    /// TUI `confirm_download` (GGUF quant group): mirror registry with
    /// zero-size entries; interactive sends (queue accounted before the
    /// sends, HUD summary per successful send, failed-send tail
    /// rollback); invalid files are skipped from the registry, reported,
    /// and still queued.
    pub fn tui_quant(base: impl Into<String>) -> Self {
        Self {
            registry: RegistryMode::Mirror {
                base: base.into(),
                entry_size: RegistryEntrySize::Zero,
            },
            discipline: SendDiscipline::Interactive,
            on_invalid: InvalidPolicy::ReportAndQueue,
        }
    }

    /// TUI repository/tree confirms: mirror registry with queued-size
    /// entries; the rest exactly as [`EnqueuePolicy::tui_quant`].
    pub fn tui_repository(base: impl Into<String>) -> Self {
        Self {
            registry: RegistryMode::Mirror {
                base: base.into(),
                entry_size: RegistryEntrySize::FromQueued,
            },
            discipline: SendDiscipline::Interactive,
            on_invalid: InvalidPolicy::ReportAndQueue,
        }
    }

    /// TUI resume: no registry writes (entries already exist), sends
    /// first with the queue accounted once after them, HUD mirror pushed
    /// unconditionally per file, no rollback, no path validation.
    pub fn tui_resume() -> Self {
        Self {
            registry: RegistryMode::AlreadyRecorded,
            discipline: SendDiscipline::Resume,
            on_invalid: InvalidPolicy::SkipValidation,
        }
    }

    /// CLI `download`: disk upsert via `registry::register_pending`
    /// (validate-first, abort on the first invalid file), queue accounted
    /// before the sends, HUD mirror pushed up front, no rollback.
    pub fn cli_download(base: impl Into<String>) -> Self {
        Self {
            registry: RegistryMode::Disk { base: base.into() },
            discipline: SendDiscipline::Batch,
            on_invalid: InvalidPolicy::AbortAll,
        }
    }

    /// `hf-cache sync`: registers NOTHING pending — the named
    /// staging-sweep decision (purge runs at bootstrap/publish, not here;
    /// see `cli/hf_cache/sync.rs`); batch sends (queue accounted before
    /// the sends, HUD mirror pushed up front, no rollback), no path
    /// validation.
    pub fn hf_cache_sync() -> Self {
        Self {
            registry: RegistryMode::StagingSweep,
            discipline: SendDiscipline::Batch,
            on_invalid: InvalidPolicy::SkipValidation,
        }
    }
}

/// What the enqueue transaction did — the input for the call site's own
/// status/error strings (which stay at the call sites).
#[derive(Debug)]
pub struct EnqueueOutcome {
    /// Files successfully handed to the download channel.
    pub sent: usize,
    /// Files whose path validation failed during mirror registry
    /// bookkeeping, in order — skipped from the registry but STILL queued
    /// and sent. The last entry is what today's call sites left in the
    /// error field.
    pub invalid: Vec<(String, crate::paths::sanitize::PathError)>,
    /// Set only by [`RegistryMode::Disk`]'s validate-first abort: nothing
    /// was queued or sent; the caller reports the error and bails.
    pub aborted: Option<crate::paths::sanitize::PathError>,
}

/// HUD summary for one queued file (the `download_queue_items` mirror).
fn queue_item_summary(file: &QueuedDownload) -> QueueItemSummary {
    QueueItemSummary {
        filename: file.filename.clone(),
        total_size: file.total_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{closed_port, EnvGuard};

    // Env-mutating tests share the crate-wide mutex from `paths`: these
    // tests redirect `RUST_HF_DOWNLOADER_DATA_DIR` (which moves the registry
    // path on every platform), the registry golden tests in `registry_tests`
    // redirect the same var — one mutex serializes every test that touches
    // the process-global registry location. (`cargo test` runs unit tests
    // in parallel threads within one process.)
    use crate::models::DownloadRegistry;
    use crate::paths::ENV_MUTEX;
    use std::path::PathBuf;

    // ---------------------------------------------------------------------
    // W2.1 `EngineState::enqueue` characterization tests. Each test pins one
    // row of the divergence table: the queue/accounting/items/rollback
    // behavior of one policy flavor, byte-for-byte as the inline code it
    // replaces behaved. Every test redirects HOME (registry path) and
    // HF_ENDPOINT (resolve_url) through the shared env guard.
    // ---------------------------------------------------------------------

    /// One queued file with the given fields (revision defaults to `main`).
    fn queued_file(filename: &str, total_size: u64) -> QueuedDownload {
        QueuedDownload {
            model_id: "a/b".to_string(),
            revision: crate::api::DEFAULT_REVISION.to_string(),
            filename: filename.to_string(),
            base_path: PathBuf::from("/tmp/base/a/b"),
            expected_sha256: Some(format!("sha-{filename}")),
            hf_token: None,
            total_size,
        }
    }

    /// Drain every message currently buffered in the download channel.
    async fn drain_downloads(state: &EngineState) -> Vec<QueuedDownload> {
        let mut rx = state.download_rx.lock().await;
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    /// Queue accounting + HUD summaries + channel contents for a
    /// successful enqueue — identical for every confirm flavor.
    async fn assert_queued(state: &EngineState, files: &[QueuedDownload]) {
        let queue = state.download_queue.lock().await;
        assert_eq!(queue.size, files.len());
        assert_eq!(queue.bytes, files.iter().map(|f| f.total_size).sum::<u64>());
        drop(queue);
        let items = state.download_queue_items.lock().await;
        let want: Vec<(&str, u64)> = files
            .iter()
            .map(|f| (f.filename.as_str(), f.total_size))
            .collect();
        let got: Vec<(&str, u64)> = items
            .iter()
            .map(|i| (i.filename.as_str(), i.total_size))
            .collect();
        assert_eq!(got, want, "HUD summaries in send order");
        drop(items);
        let drained = drain_downloads(state).await;
        assert_eq!(drained.len(), files.len());
        for (got, want) in drained.iter().zip(files) {
            assert_eq!(got.filename, want.filename);
            assert_eq!(got.model_id, want.model_id);
            assert_eq!(got.revision, want.revision);
            assert_eq!(got.base_path, want.base_path);
            assert_eq!(got.expected_sha256, want.expected_sha256);
            assert_eq!(got.total_size, want.total_size);
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_tui_quant_records_zero_size_registry_entries_and_queues_all() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("engine-enq-quant-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();
        let files = vec![queued_file("f.bin", 10), queued_file("g.bin", 20)];

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::tui_quant(tmp.to_string_lossy().to_string()),
            )
            .await;

        assert_eq!(outcome.sent, 2);
        assert!(outcome.invalid.is_empty());
        assert!(outcome.aborted.is_none());
        assert_queued(&state, &files).await;

        // GGUF quant flavor records ZERO total_size (today's quirk) and no
        // revision, with the expected sha passed through; the disk file and
        // the mirror agree (the mirror clone was saved whole).
        let mirror = state.download_registry.lock().await;
        assert_eq!(mirror.downloads.len(), 2);
        for (entry, file) in mirror.downloads.iter().zip(&files) {
            assert_eq!(entry.total_size, 0, "quant flavor records zero size");
            assert_eq!(entry.status, DownloadStatus::Incomplete);
            assert_eq!(entry.revision, None);
            assert_eq!(entry.expected_sha256, file.expected_sha256);
            assert_eq!(
                entry.url,
                crate::api::resolve_url(&file.model_id, &file.filename, &file.revision)
            );
        }
        drop(mirror);
        assert_eq!(
            crate::registry::load_registry().downloads.len(),
            2,
            "mirror clone was saved to disk"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_tui_repository_records_queued_size_registry_entries() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("engine-enq-repo-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();
        let files = vec![queued_file("sub/f.bin", 10), queued_file("g.bin", 20)];

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::tui_repository(tmp.to_string_lossy().to_string()),
            )
            .await;

        assert_eq!(outcome.sent, 2);
        assert_queued(&state, &files).await;

        // Repository/tree flavor records the QUEUED size; local_path keeps
        // the subdirectory layout under base/author/model.
        let mirror = state.download_registry.lock().await;
        assert_eq!(mirror.downloads.len(), 2);
        assert_eq!(mirror.downloads[0].total_size, 10);
        assert_eq!(mirror.downloads[1].total_size, 20);
        let local = PathBuf::from(&mirror.downloads[0].local_path);
        assert_eq!(local, tmp.join("a").join("b").join("sub").join("f.bin"));
        drop(mirror);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_mirror_dedupes_by_url_and_skips_invalid_files_but_still_queues_them() {
        use crate::paths::sanitize::PathError;

        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("engine-enq-dup-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();

        // The mirror already records f.bin's url (a previous confirm).
        let preexisting = DownloadMetadata {
            model_id: "a/b".to_string(),
            filename: "f.bin".to_string(),
            url: crate::api::resolve_url("a/b", "f.bin", crate::api::DEFAULT_REVISION),
            local_path: "/elsewhere/f.bin".to_string(),
            total_size: 999,
            downloaded_size: 0,
            status: DownloadStatus::Incomplete,
            expected_sha256: None,
            revision: None,
        };
        *state.download_registry.lock().await = DownloadRegistry {
            downloads: vec![preexisting.clone()],
        };

        // f.bin duplicates the recorded url; f2 duplicates f.bin WITHIN the
        // batch; ok.bin is new; ../evil.bin fails validation.
        let files = vec![
            queued_file("f.bin", 10),
            queued_file("f.bin", 10),
            queued_file("ok.bin", 20),
            queued_file("../evil.bin", 30),
        ];

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::tui_repository(tmp.to_string_lossy().to_string()),
            )
            .await;

        // Registry: only ok.bin was appended; the preexisting entry is
        // untouched (no overwrite of its local_path/size).
        let mirror = state.download_registry.lock().await;
        assert_eq!(mirror.downloads.len(), 2);
        assert_eq!(mirror.downloads[0].local_path, "/elsewhere/f.bin");
        assert_eq!(mirror.downloads[1].filename, "ok.bin");
        drop(mirror);

        // The invalid file is skipped from the registry but STILL queued and
        // sent, and reported for the call site's error string.
        assert_eq!(outcome.sent, 4);
        assert_eq!(outcome.invalid.len(), 1);
        assert_eq!(outcome.invalid[0].0, "../evil.bin");
        assert!(matches!(
            outcome.invalid[0].1,
            PathError::InvalidFilenameComponent(ref part) if part == ".."
        ));
        assert_queued(&state, &files).await;

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_disk_policy_aborts_on_first_invalid_file_without_queueing() {
        use crate::paths::sanitize::PathError;

        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("engine-enq-abort-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();
        let files = vec![
            queued_file("f.bin", 10),
            // Windows-reserved device stem → InvalidFilenameComponent.
            queued_file("con.gguf", 20),
            queued_file("../later.bin", 30),
        ];

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::cli_download(tmp.to_string_lossy().to_string()),
            )
            .await;

        assert!(matches!(
            outcome.aborted,
            Some(PathError::InvalidFilenameComponent(_))
        ));
        assert_eq!(outcome.sent, 0);
        assert!(outcome.invalid.is_empty());

        // Abort-first: no queue work, no HUD summaries, no sends, and the
        // disk registry was never written (validate-before-write).
        let queue = state.download_queue.lock().await;
        assert_eq!((queue.size, queue.bytes), (0, 0));
        drop(queue);
        assert!(state.download_queue_items.lock().await.is_empty());
        assert!(drain_downloads(&state).await.is_empty());
        assert!(crate::registry::load_registry().downloads.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_disk_policy_upserts_disk_registry_not_the_mirror() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("engine-enq-disk-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        // Disk already records f.bin's url → the upsert must not duplicate it.
        crate::registry::save_registry(&DownloadRegistry {
            downloads: vec![DownloadMetadata {
                model_id: "a/b".to_string(),
                filename: "f.bin".to_string(),
                url: crate::api::resolve_url("a/b", "f.bin", "deadbeef"),
                local_path: "/elsewhere/f.bin".to_string(),
                total_size: 999,
                downloaded_size: 0,
                status: DownloadStatus::Incomplete,
                expected_sha256: None,
                revision: None,
            }],
        });

        let (state, tx) = EngineState::new();
        let mut files = vec![queued_file("f.bin", 10), queued_file("g.bin", 20)];
        // Non-default revision, like `--revision deadbeef`.
        for file in &mut files {
            file.revision = "deadbeef".to_string();
        }

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::cli_download(tmp.to_string_lossy().to_string()),
            )
            .await;

        assert_eq!(outcome.sent, 2);
        assert_queued(&state, &files).await;

        // Disk: only the missing url was appended; the new entry records
        // the non-default revision (and the url was resolved against it).
        let disk = crate::registry::load_registry();
        assert_eq!(disk.downloads.len(), 2);
        assert_eq!(disk.downloads[1].filename, "g.bin");
        assert_eq!(disk.downloads[1].revision.as_deref(), Some("deadbeef"));
        assert_eq!(disk.downloads[1].total_size, 20);
        assert_eq!(
            disk.downloads[1].url,
            crate::api::resolve_url("a/b", "g.bin", "deadbeef")
        );

        // Mirror: deliberately NOT patched — the CLI seeds it from disk at
        // bootstrap, so an unseeded engine keeps its (empty) mirror.
        assert!(state.download_registry.lock().await.downloads.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Close the download channel from the receiver side so every send
    /// fails — the only way `send` on an unbounded channel can fail.
    async fn close_download_channel(state: &EngineState) {
        state.download_rx.lock().await.close();
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_failed_sends_roll_back_queue_accounting_for_tui_confirm_flavors() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("engine-enq-roll-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();
        close_download_channel(&state).await;
        let files = vec![queued_file("f.bin", 10), queued_file("g.bin", 20)];

        let outcome = state
            .enqueue(
                &tx,
                &files,
                &EnqueuePolicy::tui_repository(tmp.to_string_lossy().to_string()),
            )
            .await;

        // All sends failed → the pre-send queue.add(num, total) is rolled
        // back to zero (remove(num - sent, tail bytes), sent = 0), the HUD
        // mirror never recorded anything — identical to today's inline
        // rollback — while the registry bookkeeping STAYS (it ran first and
        // was never rolled back).
        assert_eq!(outcome.sent, 0);
        let queue = state.download_queue.lock().await;
        assert_eq!((queue.size, queue.bytes), (0, 0));
        drop(queue);
        assert!(state.download_queue_items.lock().await.is_empty());
        assert_eq!(state.download_registry.lock().await.downloads.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_resume_flavor_accounts_after_sends_and_never_rolls_back() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("engine-enq-resume-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        let (state, tx) = EngineState::new();
        close_download_channel(&state).await;
        let files = vec![queued_file("f.bin", 10), queued_file("g.bin", 20)];

        let outcome = state
            .enqueue(&tx, &files, &EnqueuePolicy::tui_resume())
            .await;

        // Resume flavor divergence, pinned: every send failed, yet the HUD
        // summaries were still pushed (one per file), the queue was
        // accounted AFTER the loop and never rolled back, and no registry
        // write happened at all (entries already exist on disk).
        assert_eq!(outcome.sent, 0);
        let queue = state.download_queue.lock().await;
        assert_eq!(queue.size, 2);
        assert_eq!(queue.bytes, 30);
        drop(queue);
        assert_eq!(state.download_queue_items.lock().await.len(), 2);
        assert!(state.download_registry.lock().await.downloads.is_empty());
        assert!(crate::registry::load_registry().downloads.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn enqueue_cli_flavors_push_items_upfront_without_rollback() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("engine-enq-cli-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = EnvGuard::install(&tmp, &format!("http://127.0.0.1:{}", closed_port()));

        // Both CLI flavors with a dead channel: HUD summaries land up front
        // (before any send), the queue accounting stays charged (no
        // rollback), and — for hf-cache — no registry write happens.
        let policies = vec![
            EnqueuePolicy::cli_download(tmp.to_string_lossy().to_string()),
            EnqueuePolicy::hf_cache_sync(),
        ];
        for policy in &policies {
            let (state, tx) = EngineState::new();
            close_download_channel(&state).await;
            let files = vec![queued_file("f.bin", 10), queued_file("g.bin", 20)];

            let outcome = state.enqueue(&tx, &files, policy).await;

            assert_eq!(outcome.sent, 0, "channel closed");
            assert!(outcome.aborted.is_none(), "valid files");
            let queue = state.download_queue.lock().await;
            assert_eq!(queue.size, 2, "CLI flavors never roll back");
            assert_eq!(queue.bytes, 30);
            drop(queue);
            assert_eq!(state.download_queue_items.lock().await.len(), 2);
            assert!(
                state.download_registry.lock().await.downloads.is_empty(),
                "hf-cache registers nothing; cli_download writes disk, not the mirror"
            );
        }

        // The cli_download flavor DID write its pending entries to disk
        // (hf-cache's pass leaves the registry file untouched).
        assert_eq!(crate::registry::load_registry().downloads.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
