//! Download-history registry persisted at
//! `crate::paths::registry_path()` (`hf-downloads.toml`): load/save the
//! [`DownloadRegistry`], query it for incomplete or completed downloads,
//! and mutate it through the typed ops below.
//!
//! # Single writer (M1)
//!
//! Every registry write runs inside [`with_registry`] under the
//! process-global `REGISTRY_WRITE` mutex (a `std` mutex — registry ops are
//! sync; brief file IO under the lock is accepted, the TOML is small).
//! The load-modify-save sequence is serialized in-process, so concurrent
//! writers (the download manager vs the verification worker, which
//! genuinely overlap — verification of one file races the next file's
//! download) can no longer lose each other's updates: every writer's
//! update survives (the R4 owner sign-off, Gate-0 2026-10-06). The two
//! former two-writer tests pinned the lost-update race as desired; they
//! now assert all-writers-win.
//!
//! `with_registry` returns the **post-write snapshot**, so callers that
//! keep an engine mirror replace it from the return value AFTER the
//! writer releases — mirror bookkeeping never happens under the write
//! lock, which is what keeps the closures leaf-only:
//!
//! - **Leaf-only closures**: a closure passed to [`with_registry`] must
//!   not call registry ops (or `with_registry`) itself — the std mutex is
//!   non-reentrant and would deadlock; a thread-local debug flag turns
//!   that re-entry into a clear debug assertion instead.
//! - **No `.await` inside**: the mutex guard is a std guard; async work
//!   (mirror patches over tokio mutexes, channel sends) belongs to the
//!   caller, after `with_registry` returns.
//! - **Disk is the source of truth**: the closure mutates the freshly
//!   loaded on-DISK registry — never a stale in-memory mirror clone.
//!
//! # Escape hatches are gone (M1)
//!
//! `load_registry`/`save_registry` are module-private: a call site cannot
//! re-compile an inline load-modify-save (the three historical bypass
//! sites — the TUI delete flow, `hf-cache` staging purge, and the enqueue
//! mirror save — were converted to typed ops). A hand-rolled
//! `fs::write`/`File::create` against the registry path outside this
//! module fails `tests/docs_guards.rs::registry_disk_writes_confined_to_registry_module`.
//! Reads need no lock and stay public as [`read_registry`]: the atomic
//! save means a concurrent reader sees the complete pre- or post-write
//! file, never a torn one.
//!
//! # Atomic save (M1, finding R5 — resolved)
//!
//! `save_registry` writes a same-directory temp file (same filesystem),
//! `sync_all()`s it, then renames it over the destination via
//! [`crate::utils::atomic_rename_with_retry`] (the sync twin — ops are
//! sync). A crash mid-save can no longer truncate `hf-downloads.toml`;
//! the rename retry absorbs Windows `rename`-over-open-file failures (a
//! reader holding the destination without share-delete),
//! docs/DEFERRED.md#registry-atomic-save. Errors stay silently
//! swallowed — the historical op behavior, pinned by the failure tests in
//! [`registry_tests`].
//!
//! # Mutation ops (W2.4b + M1)
//!
//! `upsert_pending`, `upsert_metadata`, `mark_complete` (taking a
//! [`Completion`] flavor), `register_pending`, `mark_failed` and
//! `mark_mismatch` are the single home of every registry mutation the
//! engine performs, plus the M1 bulk ops `delete_incomplete_by_urls`
//! (TUI delete flow) and `purge_staging` (`hf-cache sync`). Every op
//! keeps the exact contract of the inline code it replaced:
//!
//! 1. **Disk is the source of truth**: load the on-DISK registry, mutate
//!    it, save it — never write from the in-memory mirror.
//! 2. **The save is atomic** (temp + `sync_all` + rename-with-retry,
//!    errors silently swallowed — the historical silent-failure
//!    behavior, pinned by the golden/failure tests in `registry_tests`).
//! 3. **`mark_complete` updates its mirror after the save, regardless of
//!    whether the save succeeded** — pinned by the golden tests in
//!    `registry_tests`. `mark_mismatch` touches no mirror: the
//!    engine-mirror patch lives at its caller,
//!    `verification::mark_mismatch_mirror`, immediately after the op,
//!    independent of the save outcome.
//! 4. **Serialization is in-process only**: cross-process lost updates
//!    (concurrent CLI + TUI) remain deferred —
//!    docs/DEFERRED.md#registry-cross-process-lock.
//!
//! The byte-level behavior of every op (exact TOML after each mutation)
//! is pinned by the W2.4a golden fixtures in [`registry_tests`].

use crate::models::{CompleteDownloads, DownloadMetadata, DownloadRegistry, DownloadStatus};
use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

/// The process-global registry single writer (M1): every mutation's
/// load-modify-save runs under this mutex (see [`with_registry`]).
/// Deliberately a plain `std` mutex — ops are sync, the critical section
/// is brief file IO, and there is no await inside. Process-global on
/// purpose: path-keying (one lock per registry file) was rejected as
/// complexity — it also serializes registry writes across parallel tests
/// using distinct data dirs, which is harmless (writes are rare).
static REGISTRY_WRITE: StdMutex<()> = StdMutex::new(());

thread_local! {
    /// Whether THIS thread is currently inside `with_registry` — the
    /// leaf-only debug tripwire (a nested op on the same thread would
    /// deadlock the non-reentrant std mutex; the flag turns that into a
    /// clear debug assertion naming the contract violation).
    static IN_REGISTRY_WRITE: Cell<bool> = const { Cell::new(false) };
}

/// Run one registry mutation under the single writer (M1): lock
/// `REGISTRY_WRITE` → load the on-DISK registry → let `f` mutate it →
/// save atomically → return the **post-write snapshot**.
///
/// Contract (see the module docs): `f` is **leaf-only** — no nested
/// registry ops, no nested `with_registry` (debug-asserted; the std mutex
/// is non-reentrant) — and **synchronous** — never `.await` inside (the
/// guard is a std mutex guard; async work belongs to the caller after
/// this returns). Callers that keep an engine mirror replace it from the
/// returned snapshot after the writer releases — never patch the mirror
/// inside `f`.
///
/// Brief file IO under the lock is accepted (the registry TOML is small);
/// save errors stay silently swallowed, exactly like every legacy op.
pub fn with_registry(f: impl FnOnce(&mut DownloadRegistry)) -> DownloadRegistry {
    debug_assert!(
        !IN_REGISTRY_WRITE.with(Cell::get),
        "with_registry closures are leaf-only: a nested registry op would \
         deadlock the non-reentrant REGISTRY_WRITE mutex"
    );
    let _writer = REGISTRY_WRITE.lock().unwrap_or_else(|e| e.into_inner());
    IN_REGISTRY_WRITE.with(|held| held.set(true));
    let mut registry = load_registry();
    f(&mut registry);
    save_registry(&registry);
    IN_REGISTRY_WRITE.with(|held| held.set(false));
    registry
}

/// Read the on-disk registry (the public read API — bootstrap mirror
/// seeding, tests asserting disk state). Reads take no lock: the atomic
/// save means a concurrent reader sees the complete pre- or post-write
/// file, never a torn one. Missing/unparseable file → empty registry
/// (the historical load behavior).
pub fn read_registry() -> DownloadRegistry {
    load_registry()
}

/// Load the registry from disk. Module-private (M1): together with the
/// private [`save_registry`] this closes the inline load-modify-save
/// escape hatch — every mutation must go through [`with_registry`] and
/// the typed ops. Public reads go through [`read_registry`].
fn load_registry() -> DownloadRegistry {
    let path = crate::paths::registry_path();
    if !path.exists() {
        return DownloadRegistry::default();
    }

    match fs::read_to_string(&path) {
        Ok(content) => toml::from_str(&content).unwrap_or_default(),
        Err(_) => DownloadRegistry::default(),
    }
}

/// Persist the registry ATOMICALLY (M1, finding R5): same-directory temp
/// file (same filesystem) → `write_all` → `sync_all()` → rename over the
/// destination via [`crate::utils::atomic_rename_with_retry`]. A crash
/// mid-save can no longer truncate `hf-downloads.toml` (the old
/// `File::create` write truncated first). The retry matters on Windows:
/// `rename` over a destination a concurrent reader holds without
/// FILE_SHARE_DELETE fails transiently, and the bounded retry absorbs
/// exactly those windows (AV/indexer — same policy as the download
/// pipeline's final rename). Module-private: this is the single write
/// path, called only from [`with_registry`]. Errors stay silently
/// swallowed (the historical op behavior, pinned by the failure tests in
/// `registry_tests`).
fn save_registry(registry: &DownloadRegistry) {
    let path = crate::paths::registry_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let Ok(toml_string) = toml::to_string_pretty(registry) else {
        return;
    };

    // Same-dir temp file (rename must not cross filesystems); the pid
    // suffix keeps a concurrent process from clobbering our temp.
    let temp_path = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    let written = fs::File::create(&temp_path)
        .and_then(|mut file| {
            file.write_all(toml_string.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| {
            // Sync twin on purpose: `with_registry` is sync, and this save
            // runs under its std mutex — no await points in sight.
            crate::utils::atomic_rename_with_retry(
                &temp_path,
                &path,
                RENAME_RETRIES,
                RENAME_RETRY_DELAY,
            )
        });
    if written.is_err() {
        // Best effort cleanup so a failed save litters no temp file
        // (silently swallowed, like every legacy registry error).
        let _ = fs::remove_file(&temp_path);
    }
}

/// Registry-save rename policy: 1 initial try + 4 retries, 100 ms base
/// delay with linear backoff — the download pipeline's final-rename
/// policy (incident #37 symptom B), reused so both Windows-sensitive
/// renames in the crate share one tuned budget.
const RENAME_RETRIES: u32 = 4;
const RENAME_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Typed mutation ops (W2.4b + M1)
//
// Each op is the extracted inline load-modify-save sequence of one engine
// site (see the module docs for the shared contract). Since M1 every op
// runs inside `with_registry` — the process-global single writer — so the
// load-mutate-save is serialized and the save is atomic. The exact bytes
// each op writes are pinned by the golden fixtures in `registry_tests` —
// those assertions passed unchanged through the W2.4b migration (inline →
// ops) and the M1 migration (ops → single-writer ops).
// ---------------------------------------------------------------------------

/// Seed the on-disk registry with `Incomplete` entries for files about to
/// be queued, so downloads started headlessly show up in the TUI's
/// resume/complete views (moved here from `engine.rs`: pending writes have
/// one owner, next to the `upsert_pending` op they drive). Validates each
/// filename (path-traversal safety, same rules as the TUI) and returns the
/// first validation error, if any. The error type is the shared
/// [`crate::paths::sanitize::PathError`]: path validation is
/// register_pending's only failure source today, and its `Display`
/// reproduces the historical message strings byte-for-byte.
pub fn register_pending(
    model_id: &str,
    revision: &str,
    files: &[(String, u64, Option<String>)],
    base_path: &str,
) -> Result<(), crate::paths::sanitize::PathError> {
    // Validate and build every entry first: the first invalid filename
    // aborts (via `?`) before anything is written — no partial save. The
    // registry write itself is the shared `upsert_pending` op (one load,
    // append-only-missing-urls, one save).
    let mut entries = Vec::with_capacity(files.len());
    for (filename, size, sha256) in files {
        let validated_path =
            crate::paths::sanitize::validate_and_sanitize_path(base_path, model_id, filename)?;

        let url = crate::api::resolve_url(model_id, filename, revision);
        entries.push(DownloadMetadata {
            model_id: model_id.to_string(),
            filename: filename.clone(),
            url,
            local_path: validated_path.to_string_lossy().to_string(),
            total_size: *size,
            downloaded_size: 0,
            status: DownloadStatus::Incomplete,
            expected_sha256: sha256.clone(),
            revision: if revision == crate::api::DEFAULT_REVISION {
                None
            } else {
                Some(revision.to_string())
            },
        });
    }

    upsert_pending(&entries);
    Ok(())
}

/// Seed the registry with `Incomplete` entries for files about to be
/// queued ([`register_pending`] — and, since M1, the TUI confirm flows'
/// enqueue arm): one load, one append per entry whose url is not recorded
/// yet, one save — under the single writer. Callers validate filenames
/// first — this op performs no path validation and no per-entry aborts.
/// Returns the post-write snapshot so mirror-keeping callers replace
/// their mirror from it after the writer releases.
pub fn upsert_pending(entries: &[DownloadMetadata]) -> DownloadRegistry {
    with_registry(|registry| {
        for entry in entries {
            if !registry.downloads.iter().any(|d| d.url == entry.url) {
                registry.downloads.push(entry.clone());
            }
        }
    })
}

/// Record a download's metadata before its chunks start
/// (`download::download_chunked`): an entry with a known url is refreshed
/// (`total_size` updated, `downloaded_size` reset to 0 — nothing else is
/// touched); an unknown url appends `entry` verbatim.
pub fn upsert_metadata(entry: DownloadMetadata) {
    with_registry(|registry| {
        if let Some(existing) = registry.downloads.iter_mut().find(|d| d.url == entry.url) {
            existing.total_size = entry.total_size;
            existing.downloaded_size = 0;
        } else {
            registry.downloads.push(entry);
        }
    });
}

/// How a download finished — the input shape of [`mark_complete`]. The
/// two former ops (`mark_complete` / `mark_complete_with_url`) differed
/// in three correlated ways (match predicate, `downloaded_size`, url
/// rewrite); one enum keeps each flavor's exact semantics while making
/// the wrong mixtures unrepresentable.
#[derive(Debug, Clone, Copy)]
pub enum Completion<'a> {
    /// The file already existed on disk (`download::start_download`'s
    /// already-exists path): the entry matching `url` flips to
    /// `Complete` — its `downloaded_size` and every other field are
    /// left alone.
    AlreadyExists {
        /// Registry url of the download (the resolve URL).
        url: &'a str,
    },
    /// The chunked download finished (`download::start_download`'s
    /// success path): the entry matching `url` OR `successful_url` (the
    /// raw-endpoint fallback) flips to `Complete` with
    /// `downloaded_size` set and its url rewritten to the successful
    /// one.
    Downloaded {
        /// Registry url of the download (the resolve URL).
        url: &'a str,
        /// The URL that actually served the bytes (may equal `url`).
        successful_url: &'a str,
        /// Final byte count recorded on the entry.
        downloaded_size: u64,
    },
}

/// Mark a download complete (`download::start_download`'s two success
/// paths — see [`Completion`] for each flavor's exact rules): the
/// matching entry flips to `Complete`, the mutated registry is saved
/// (atomically, under the single writer), and the mutated entry is
/// inserted into the complete-downloads mirror — AFTER the writer
/// released (the tokio mirror lock is never taken under the std write
/// mutex). No entry on disk means no change and no mirror insert.
pub async fn mark_complete(
    complete_downloads: &Arc<Mutex<CompleteDownloads>>,
    completion: Completion<'_>,
    filename: &str,
) {
    let mut updated = None;
    with_registry(|registry| match completion {
        Completion::AlreadyExists { url } => {
            if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
                entry.status = DownloadStatus::Complete;
                updated = Some(entry.clone());
            }
        }
        Completion::Downloaded {
            url,
            successful_url,
            downloaded_size,
        } => {
            if let Some(entry) = registry
                .downloads
                .iter_mut()
                .find(|d| d.url == url || d.url == successful_url)
            {
                entry.status = DownloadStatus::Complete;
                entry.downloaded_size = downloaded_size;
                entry.url = successful_url.to_string();
                updated = Some(entry.clone());
            }
        }
    });

    if let Some(entry) = updated {
        let mut complete = complete_downloads.lock().await;
        complete.insert(filename.to_string(), entry);
    }
}

/// Mark a failed download (`download::start_download` 401 and
/// final-failure paths — both run the identical sequence): the entry
/// matching `url` drops to `Incomplete` with `downloaded_size = 0`. No
/// mirror update.
pub fn mark_failed(url: &str) {
    with_registry(|registry| {
        if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
            entry.status = DownloadStatus::Incomplete;
            entry.downloaded_size = 0;
        }
    });
}

/// Whether a registry-recorded path string refers to the same file as
/// `actual`. Registry entries record the user-facing path (original base,
/// e.g. `/var/...` on macOS or `C:\...` on Windows) while download
/// internals canonicalize (`/private/var/...`, `\\?\C:\...`), so raw
/// string equality fails cross-platform. Canonicalize both sides when the
/// raw forms differ; falls back to `false` when either side cannot be
/// resolved.
pub(crate) fn path_matches(recorded: &str, actual: &Path) -> bool {
    if recorded == actual.to_string_lossy() {
        return true;
    }
    match (Path::new(recorded).canonicalize(), actual.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Record a SHA256 mismatch (`verification::verify_file`): the disk entry
/// whose `local_path` matches flips to `HashMismatch` and is saved.
/// Pure disk op: the engine's in-memory registry mirror is patched by the
/// CALLER — `verification::mark_mismatch_mirror` — immediately after this
/// op returns, regardless of whether the save succeeded (the mirror may
/// lack the entry entirely; the disk is the source of truth).
pub fn mark_mismatch(local_path: &Path) {
    with_registry(|registry| {
        if let Some(entry) = registry
            .downloads
            .iter_mut()
            .find(|d| path_matches(&d.local_path, local_path))
        {
            entry.status = DownloadStatus::HashMismatch;
        }
    });
}

/// Remove every registry entry whose url is in `urls` (the TUI's delete
/// flow, `ui/app/downloads.rs::delete_incomplete_downloads` — the inline
/// mirror-clone save this op replaces was the R1 bypass site). Loads the
/// on-DISK registry — never the in-memory mirror — drops the selected
/// urls, saves atomically under the single writer, and returns the
/// post-write snapshot so the caller replaces the engine mirror from it.
/// Whether the corresponding `.incomplete` files were actually deleted
/// is the caller's concern; this op only owns the registry rows.
pub fn delete_incomplete_by_urls(urls: &[String]) -> DownloadRegistry {
    with_registry(|registry| {
        registry.downloads.retain(|d| !urls.contains(&d.url));
    })
}

/// Drop every on-disk registry entry whose `local_path` lives in a
/// `.rhd-staging` directory (the `hf-cache sync` hygiene step — the
/// inline load-modify-save this op replaces was the R3 bypass site).
/// Returns `None` when nothing matches — the legacy conditional-save
/// contract: no write happens at all, so a missing registry file is not
/// materialized as an empty one. The `contains(".rhd-staging")`
/// predicate is byte-identical to the inline code (a substring match, not
/// a path-component match), pinned by the TOML golden in
/// `cli/hf_cache/sync.rs::tests`. Best-effort, like the inline code:
/// errors are silently swallowed.
pub fn purge_staging() -> Option<DownloadRegistry> {
    // Pre-check outside the writer (reads take no lock — see the module
    // docs): skip the serialized write entirely when there is nothing to
    // purge. The predicate below is duplicated deliberately — pre-check
    // plus under-lock mutation must stay the same string.
    if !read_registry()
        .downloads
        .iter()
        .any(|d| d.local_path.contains(".rhd-staging"))
    {
        return None;
    }
    Some(with_registry(|registry| {
        registry
            .downloads
            .retain(|d| !d.local_path.contains(".rhd-staging"));
    }))
}

pub fn get_incomplete_downloads(
    registry: &DownloadRegistry,
) -> Vec<crate::models::DownloadMetadata> {
    registry
        .downloads
        .iter()
        .filter(|d| {
            d.status == DownloadStatus::Incomplete || d.status == DownloadStatus::HashMismatch
        })
        .cloned()
        .collect()
}

pub fn get_complete_downloads(
    registry: &DownloadRegistry,
) -> std::collections::HashMap<String, crate::models::DownloadMetadata> {
    registry
        .downloads
        .iter()
        .filter(|d| d.status == DownloadStatus::Complete)
        .map(|d| (d.filename.clone(), d.clone()))
        .collect()
}

/// W2.4 golden + contract tests for the registry mutation ops (see the
/// module docs inside).
#[cfg(test)]
mod registry_tests;
