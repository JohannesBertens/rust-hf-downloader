//! Download-history registry persisted at
//! `crate::paths::registry_path()` (`hf-downloads.toml`): load/save the
//! [`DownloadRegistry`], query it for incomplete or completed downloads,
//! and mutate it through the typed ops below.
//!
//! # Mutation ops (W2.4b)
//!
//! `upsert_pending`, `upsert_metadata`, `mark_complete` (taking a
//! [`Completion`] flavor), `register_pending`, `mark_failed` and
//! `mark_mismatch` are the
//! single home of every registry mutation the engine performs (the inline
//! load-modify-save sequences they replaced lived in `download.rs`,
//! `verification.rs` and `engine.rs`). Every op keeps the exact contract
//! of the inline code it replaced:
//!
//! 1. **Disk is the source of truth**: load the on-DISK registry, mutate
//!    it, save it — never write from the in-memory mirror.
//! 2. **The save stays non-atomic** (`fs::File::create` semantics, errors
//!    silently swallowed) — making it atomic is deliberately deferred
//!    (plan §8.5); crash-truncation behavior is unchanged.
//! 3. **`mark_complete` updates its mirror after the save, regardless of
//!    whether the save succeeded** — today's silent-failure behavior,
//!    pinned by the golden tests in `registry_tests`. `mark_mismatch` no
//!    longer touches any mirror (final layering pass): the engine-mirror
//!    patch moved to its caller, `verification::mark_mismatch_mirror`, so
//!    this module is pure disk ops. Its timing contract — immediately
//!    after the op, independent of the save outcome — is pinned by the
//!    verification-side tests.
//! 4. **No lock is held across load-modify-save.** The lost-update race
//!    between concurrent writers is a known deferred defect (plan §8);
//!    the ops must not add cross-op serialization.
//!
//! The byte-level behavior of every op (exact TOML after each mutation)
//! is pinned by the W2.4a golden fixtures in [`registry_tests`].

use crate::models::{CompleteDownloads, DownloadMetadata, DownloadRegistry, DownloadStatus};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

pub fn load_registry() -> DownloadRegistry {
    let path = crate::paths::registry_path();
    if !path.exists() {
        return DownloadRegistry::default();
    }

    match fs::read_to_string(&path) {
        Ok(content) => toml::from_str(&content).unwrap_or_default(),
        Err(_) => DownloadRegistry::default(),
    }
}

pub fn save_registry(registry: &DownloadRegistry) {
    let path = crate::paths::registry_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Ok(toml_string) = toml::to_string_pretty(registry) {
        if let Ok(mut file) = fs::File::create(&path) {
            let _ = file.write_all(toml_string.as_bytes());
        }
    }
}

// ---------------------------------------------------------------------------
// Typed mutation ops (W2.4b)
//
// Each op is the extracted inline load-modify-save sequence of one engine
// site (see the module docs for the shared contract). The exact bytes each
// op writes are pinned by the golden fixtures in `registry_tests` — those
// assertions passed unchanged through the migration from the inline code.
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
/// queued ([`register_pending`]): one load, one append per entry
/// whose url is not recorded yet, one save. Callers validate filenames
/// first — this op performs no path validation and no per-entry aborts.
pub fn upsert_pending(entries: &[DownloadMetadata]) {
    let mut registry = load_registry();
    for entry in entries {
        if !registry.downloads.iter().any(|d| d.url == entry.url) {
            registry.downloads.push(entry.clone());
        }
    }
    save_registry(&registry);
}

/// Record a download's metadata before its chunks start
/// (`download::download_chunked`): an entry with a known url is refreshed
/// (`total_size` updated, `downloaded_size` reset to 0 — nothing else is
/// touched); an unknown url appends `entry` verbatim.
pub fn upsert_metadata(entry: DownloadMetadata) {
    let mut registry = load_registry();
    if let Some(existing) = registry.downloads.iter_mut().find(|d| d.url == entry.url) {
        existing.total_size = entry.total_size;
        existing.downloaded_size = 0;
    } else {
        registry.downloads.push(entry);
    }
    save_registry(&registry);
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
/// matching entry flips to `Complete`, the mutated registry is saved,
/// and the mutated entry is inserted into the complete-downloads
/// mirror. No entry on disk means no change and no mirror insert.
pub async fn mark_complete(
    complete_downloads: &Arc<Mutex<CompleteDownloads>>,
    completion: Completion<'_>,
    filename: &str,
) {
    let mut registry = load_registry();
    let mut updated = None;
    match completion {
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
    }
    save_registry(&registry);

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
    let mut registry = load_registry();
    if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
        entry.status = DownloadStatus::Incomplete;
        entry.downloaded_size = 0;
    }
    save_registry(&registry);
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
/// Pure disk op (final layering pass): the engine's in-memory registry
/// mirror is patched by the CALLER — `verification::mark_mismatch_mirror`
/// — immediately after this op returns, regardless of whether the save
/// succeeded (the mirror may lack the entry entirely; the disk is the
/// source of truth).
pub fn mark_mismatch(local_path: &Path) {
    let mut registry = load_registry();
    if let Some(entry) = registry
        .downloads
        .iter_mut()
        .find(|d| path_matches(&d.local_path, local_path))
    {
        entry.status = DownloadStatus::HashMismatch;
    }
    save_registry(&registry);
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
