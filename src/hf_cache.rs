//! HuggingFace hub-cache layout writer for `hf-cache sync`
//! (plans/hf-cache-sync.md §3, §4.3, §5.1).
//!
//! This module owns the on-disk shape of the real HuggingFace hub cache so
//! that `vllm serve`, transformers, and `hf download` find a byte-identical,
//! revision-pinned snapshot without any network calls:
//!
//! ```text
//! <cache>/models--a--b/
//!     refs/<branch>            # resolved commit SHA (R2)
//!     blobs/<oid>              # LFS: sha256 | non-LFS: git blob sha1 (R1)
//!     snapshots/<sha>/<path>   # relative symlink → ../../blobs/<oid> (R3)
//!     .rhd-staging/<path>      # engine working dir (§5.1), renamed into
//!                              # blobs/ atomically at publish time (R5)
//! ```
//!
//! The split mirrors §4.3: [`plan`] decides what to fetch using
//! `metadata()` calls only (never reads file contents), while
//! [`publish_one`] performs the atomic staging→blob rename plus the
//! snapshot entry, falling back to a copy when symlinks are unavailable
//! (R4). Re-running a sync is idempotent (R6): blobs already present with
//! matching sizes are skipped; `--force` refetches them.
//!
//! Driven by `cli::hf_cache_cmd`; unit tests exercise the pipeline directly.

use crate::models::RepoFile;
use crate::utils::atomic_rename_with_retry;
use sha1::{Digest, Sha1};
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Hub cache directory prefix: `<org>/<name>` becomes
/// `models--<org>--<name>`.
const REPO_DIR_PREFIX: &str = "models--";

/// Hub cache subdirectory holding blob content addressed by oid.
const BLOBS_DIR: &str = "blobs";

/// Hub cache subdirectory holding per-commit snapshot trees.
const SNAPSHOTS_DIR: &str = "snapshots";

/// Hub cache subdirectory holding resolved branch/tag → commit-SHA files.
const REFS_DIR: &str = "refs";

/// Cache-root subdirectory holding huggingface_hub's per-file publish
/// locks (§5.2 step 6). Exact hub naming is verified against an installed
/// hub client in M4 (§11.2).
const LOCKS_DIR: &str = ".locks";

/// Engine working directory inside a repo folder (§5.1): downloads land
/// here and are renamed into `blobs/` on publish — same filesystem, so
/// the rename is instant and atomic.
const STAGING_DIR_NAME: &str = ".rhd-staging";

/// Name of the per-repo sync lock file inside the staging directory
/// (§5.2 step 6). Serializes concurrent `hf-cache sync` runs on the same
/// repo.
const SYNC_LOCK_FILE: &str = ".sync.lock";

/// A sync lock older than this is considered abandoned and stolen on the
/// next acquire (first-guess policy; §5.2 step 6, observed before freezing
/// per §11.4).
const STALE_SYNC_LOCK: Duration = Duration::from_secs(24 * 60 * 60);

// ---------------------------------------------------------------------------
// Pure layout math
// ---------------------------------------------------------------------------

/// Hub directory name for a model id: `/` becomes `--` (R1/R3 layout
/// root). `"a/b"` → `"models--a--b"`; an id without a namespace →
/// `"models--<id>"`.
pub fn repo_dir_name(model_id: &str) -> String {
    format!("{REPO_DIR_PREFIX}{}", model_id.replace('/', "--"))
}

/// Computes the git blob sha1 of a file: `sha1("blob <len>\0" ++ content)`.
/// This is the blob name for non-LFS files whose tree entry lacks `oid`
/// (R1 fallback), and matches `git hash-object` output.
///
/// Streams the file through the shared digest core (64 KiB reads) instead
/// of loading it whole; `<len>` comes from `fs::metadata` taken up front.
/// Plan amendment (W1.6): if the file changes size while being read
/// (bytes read ≠ stat length), this returns an error instead of producing
/// a digest — the previous whole-file read could never observe that race
/// but also silently hashed whatever torn interleaving it saw; erroring is
/// the safer contract for a cache blob name.
pub fn git_blob_sha1(path: &Path) -> io::Result<String> {
    let len = fs::metadata(path)?.len();
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {len}\0").as_bytes());
    let hashed =
        crate::utils::stream_file_digest(path, &mut hasher, crate::utils::DIGEST_CHUNK, |_, _| {})?;
    if hashed != len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("file changed size while hashing: read {hashed} bytes, metadata said {len}"),
        ));
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Snapshot directory `<cache>/models--<org>--<name>/snapshots/<sha>` for
/// a resolved commit SHA (R3).
pub fn snapshot_dir(cache: &Path, model_id: &str, sha: &str) -> PathBuf {
    cache
        .join(repo_dir_name(model_id))
        .join(SNAPSHOTS_DIR)
        .join(sha)
}

/// Relative symlink target for a snapshot entry (R3): `../../blobs/<oid>`
/// for top-level repo paths, one more `../` per directory component of
/// the repo path (`snapshots/<sha>/sub/file` → `../../../blobs/<oid>`),
/// mirroring hub's `os.path.relpath`-computed links. Relative so the
/// cache works regardless of where it is mounted — critical for
/// containers and NFS.
pub fn snapshot_symlink_target(oid: &str, repo_path: &str) -> String {
    let depth = 2 + repo_path.matches('/').count();
    format!("{}{BLOBS_DIR}/{oid}", "../".repeat(depth))
}

/// Engine staging directory `<repo_dir>/.rhd-staging` (§5.1). Downloads
/// write here; publishing renames into `blobs/` on the same filesystem.
pub fn staging_dir(repo_dir: &Path) -> PathBuf {
    repo_dir.join(STAGING_DIR_NAME)
}

// ---------------------------------------------------------------------------
// Planning (R6 idempotency)
// ---------------------------------------------------------------------------

/// One file to download into staging, then publish into the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchItem {
    /// Repo-relative POSIX path (e.g. `subdir/model-00001-of-00002.safetensors`).
    pub repo_path: String,
    /// Blob name this file publishes under (R1): LFS sha256 or git blob
    /// sha1. `None` when the tree entry carries neither — resolved at
    /// publish time by hashing the staged content (`git_blob_sha1`).
    pub blob_oid: Option<String>,
    /// Expected byte size from the tree entry; `0` when the entry carries
    /// none.
    pub size: u64,
    /// LFS sha256 when the file is LFS-tracked — the digest the
    /// verification worker checks before publishing (R5). `None` for
    /// non-LFS files (no hub-side digest to verify).
    pub sha256: Option<String>,
}

/// What a sync needs to do against the current cache contents (§5.2
/// step 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPlan {
    /// Resolved commit SHA the plan snapshots.
    pub sha: String,
    /// Files to fetch: blob missing, size-mismatched, or `--force`.
    pub fetch: Vec<FetchItem>,
    /// Repo paths whose blob is already present with a matching size (R6).
    pub up_to_date: Vec<String>,
}

/// Decides what to fetch for `selected` paths of a repo tree at commit
/// `sha` (R6): a path whose blob `blobs/<oid>` already exists **with a
/// matching size** lands in `up_to_date` (unless `force`); everything
/// else becomes a [`FetchItem`]. Uses only `metadata()` calls — file
/// contents are never read, so planning stays cheap and side-effect free.
///
/// Blob oids come from the tree entries (`lfs.oid` else `oid`, R1). An
/// entry carrying neither keeps the sync alive: the blob name is
/// resolved at publish time by hashing the staged content (R1 fallback),
/// and such files can never be `up_to_date` (the blob name is unknown at
/// plan time). Duplicate tree entries for one path must agree on oid and
/// size, else the plan is rejected (E2 in §9).
pub fn plan(
    cache: &Path,
    model_id: &str,
    tree: &[RepoFile],
    selected: &[String],
    sha: &str,
    force: bool,
) -> io::Result<SyncPlan> {
    let blobs_dir = cache.join(repo_dir_name(model_id)).join(BLOBS_DIR);
    let mut fetch = Vec::with_capacity(selected.len());
    let mut up_to_date = Vec::new();
    for path in selected {
        validate_relative_path(path)?;
        // E2: duplicate rfilenames across recursive tree walks — last
        // entry wins, but all copies must agree on oid and size.
        let matches: Vec<&RepoFile> = tree.iter().filter(|f| f.rfilename == *path).collect();
        let entry = matches.last().copied().ok_or_else(|| {
            invalid_input(format!("selected path not present in repo tree: {path}"))
        })?;
        if matches.iter().any(|f| {
            tree_blob_oid(f) != tree_blob_oid(entry) || effective_size(f) != effective_size(entry)
        }) {
            return Err(invalid_input(format!(
                "conflicting duplicate tree entries for {path}"
            )));
        }
        let blob_oid = tree_blob_oid(entry);
        let expected = effective_size(entry);
        match blob_oid {
            // R6: only oid-known blobs can be recognized as current.
            Some(oid) if !force && blob_is_current(&blobs_dir.join(&oid), expected) => {
                up_to_date.push(path.clone());
            }
            _ => fetch.push(FetchItem {
                repo_path: path.clone(),
                blob_oid,
                size: expected.unwrap_or(0),
                sha256: entry.lfs.as_ref().map(|l| l.oid.clone()),
            }),
        }
    }
    Ok(SyncPlan {
        sha: sha.to_string(),
        fetch,
        up_to_date,
    })
}

/// Blob oid for a tree entry without touching any file content: LFS oid
/// (sha256) wins over the tree `oid` (R1). `None` when the entry carries
/// neither.
fn tree_blob_oid(file: &RepoFile) -> Option<String> {
    file.lfs
        .as_ref()
        .map(|l| l.oid.clone())
        .or_else(|| file.oid.clone())
}

/// Expected on-disk size of a tree entry: the sibling `size` when present
/// (the hub sets it to the real content size, also for LFS), else the LFS
/// pointer's size.
fn effective_size(file: &RepoFile) -> Option<u64> {
    file.size.or_else(|| file.lfs.as_ref().map(|l| l.size))
}

/// Whether a blob file is provably current: a regular file whose length
/// equals the expected size. Unknown expected size (`None`) is *not*
/// provably current — plan conservatively re-fetches (§5.3); verification
/// gates the publish anyway (R5).
fn blob_is_current(blob: &Path, expected_size: Option<u64>) -> bool {
    match (fs::metadata(blob), expected_size) {
        (Ok(md), Some(want)) => md.is_file() && md.len() == want,
        (Ok(_), None) => false,
        (Err(_), _) => false,
    }
}

// ---------------------------------------------------------------------------
// Publishing (R3/R4/R5)
// ---------------------------------------------------------------------------

/// Publishes one fully downloaded and verified file into the cache (R5),
/// returning the resolved blob oid:
///
/// 1. resolve the blob name (R1): `item.blob_oid`, or — when the tree
///    carried no oid — the git blob sha1 computed from the staged
///    content; for non-LFS files with a known oid the sha1 is *verified*
///    against the staged content first (bad bytes never enter `blobs/`),
/// 2. rename `staged_path` → `blobs/<oid>` — atomic because
///    staging lives inside the repo dir, so source and destination share a
///    filesystem (§5.1); concurrent readers never observe partial files,
/// 3. create the snapshot entry `snapshots/<sha>/<repo_path>` as a
///    depth-aware relative symlink into `../../blobs/<oid>` (R3),
///    replacing any existing (dangling or stale) entry,
/// 4. when symlinks are disabled or creation fails (Windows without dev
///    mode), copy the blob into the snapshot path instead (R4), with a
///    one-time warning when the fallback was not requested.
///
/// A best-effort hub-style per-file lock
/// `.locks/<repo-dir>/<repo_path-with-underscores>.lock` is held for the
/// duration of the call (§5.2 step 6); lock failures never block the
/// publish — the rename protocol itself guarantees cache integrity. The
/// exact hub lock filename is verified against an installed hub client in
/// M4 (§11.2).
///
/// Note: `sha` is passed separately from [`FetchItem`] — the item is
/// repo-scoped (reusable across revisions), while the snapshot directory
/// is commit-specific.
pub fn publish_one(
    repo_dir: &Path,
    sha: &str,
    item: &FetchItem,
    staged_path: &Path,
    use_symlinks: bool,
) -> io::Result<String> {
    validate_relative_path(&item.repo_path)?;
    let _lock = HubFileLock::acquire(repo_dir, &item.repo_path);

    // R1: resolve the blob name, verifying non-LFS content against the
    // tree's git blob sha1 when we have one (the engine's SHA256 gate
    // covers LFS files; this closes the loop for plain git files).
    let blob_oid = match (&item.blob_oid, &item.sha256) {
        (Some(oid), None) => {
            let actual = git_blob_sha1(staged_path)?;
            if *oid != actual {
                return Err(invalid_input(format!(
                    "content of {} does not match tree oid (expected {oid}, got {actual})",
                    item.repo_path
                )));
            }
            actual
        }
        (Some(oid), Some(_)) => oid.clone(),
        (None, _) => git_blob_sha1(staged_path)?,
    };

    // R5: staging lives inside repo_dir, so this rename stays on one
    // filesystem — instant and atomic.
    let blob_path = repo_dir.join(BLOBS_DIR).join(&blob_oid);
    fs::create_dir_all(repo_dir.join(BLOBS_DIR))?;
    rename_replacing(staged_path, &blob_path)?;

    // R3/R4: snapshot entry — relative symlink, copy fallback.
    let snapshot_path = repo_dir.join(SNAPSHOTS_DIR).join(sha).join(&item.repo_path);
    if let Some(parent) = snapshot_path.parent() {
        fs::create_dir_all(parent)?; // nested repo paths need their dirs (R3)
    }
    remove_existing_entry(&snapshot_path)?;
    let target = snapshot_symlink_target(&blob_oid, &item.repo_path);
    if use_symlinks {
        match create_symlink(&target, &snapshot_path) {
            Ok(()) => return Ok(blob_oid),
            Err(e) => warn_symlink_fallback_once(&snapshot_path, &e),
        }
    }
    // A failed symlink attempt leaves no entry behind, but be safe before
    // the copy creates a fresh one.
    let _ = remove_existing_entry(&snapshot_path);
    fs::copy(&blob_path, &snapshot_path)?;
    Ok(blob_oid)
}

/// R4 wants the symlink→copy fallback warning printed once per process,
/// not once per file.
fn warn_symlink_fallback_once(snapshot_path: &Path, e: &io::Error) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "warning: symlink creation failed ({}); copying files into snapshots/ instead (hub degraded mode). First affected file: {}",
            e,
            snapshot_path.display()
        );
    }
}

/// Idempotently ensures the snapshot entry `snapshots/<sha>/<repo_path>`
/// for a blob that is **already present** (R6/E9): a missing, dangling, or
/// stale entry is (re)created — relative symlink with copy fallback (R4) —
/// while a correct existing entry is left untouched so re-runs stay cheap.
/// Unlike [`publish_one`] no staging file is consumed: the blob already
/// sits in `blobs/`.
///
/// The file's blob oid follows R1 precedence (LFS sha256 → tree oid);
/// an entry carrying neither is an error, exactly as in [`plan`].
pub fn ensure_snapshot_entry(
    repo_dir: &Path,
    sha: &str,
    file: &RepoFile,
    use_symlinks: bool,
) -> io::Result<()> {
    validate_relative_path(&file.rfilename)?;
    let blob_oid = tree_blob_oid(file).ok_or_else(|| {
        invalid_input(format!("tree entry for {} carries no oid", file.rfilename))
    })?;
    let snapshot_path = repo_dir.join(SNAPSHOTS_DIR).join(sha).join(&file.rfilename);
    if let Some(parent) = snapshot_path.parent() {
        fs::create_dir_all(parent)?; // nested repo paths need their dirs (R3)
    }
    let target = snapshot_symlink_target(&blob_oid, &file.rfilename);

    if !use_symlinks {
        // Copy mode: an existing regular entry is assumed current (the
        // same assumption hub makes); only a missing or foreign entry is
        // (re)written.
        if fs::symlink_metadata(&snapshot_path).is_ok_and(|md| md.is_file()) {
            return Ok(());
        }
        remove_existing_entry(&snapshot_path)?;
        return fs::copy(repo_dir.join(BLOBS_DIR).join(&blob_oid), &snapshot_path).map(|_| ());
    }

    // Symlink mode: a link already pointing at the right blob is a no-op.
    if let (Ok(md), Ok(link)) = (
        fs::symlink_metadata(&snapshot_path),
        fs::read_link(&snapshot_path),
    ) {
        if md.file_type().is_symlink() && link == Path::new(&target) {
            return Ok(());
        }
    }
    remove_existing_entry(&snapshot_path)?;
    if create_symlink(&target, &snapshot_path).is_ok() {
        return Ok(());
    }
    let _ = remove_existing_entry(&snapshot_path);
    fs::copy(repo_dir.join(BLOBS_DIR).join(&blob_oid), &snapshot_path).map(|_| ())
}

/// `std::fs::rename` that replaces an existing destination file: Unix
/// `rename(2)` already clobbers atomically; Windows refuses, so the
/// destination is removed and the rename retried.
///
/// Each raw rename delegates to [`atomic_rename_with_retry`] with
/// `retries = 0` — hub-cache publishing keeps its exact single-attempt
/// semantics; the transient-lock retry policy belongs to the download
/// pipeline's call site, not here.
fn rename_replacing(from: &Path, to: &Path) -> io::Result<()> {
    match atomic_rename_with_retry(from, to, 0, std::time::Duration::ZERO) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(to);
            atomic_rename_with_retry(from, to, 0, std::time::Duration::ZERO)
        }
        Err(e) => Err(e),
    }
}

/// Removes whatever sits at `path` — regular file, symlink (dangling or
/// not; never follows the link), or directory — so a fresh snapshot entry
/// can take its place. Missing paths are fine.
fn remove_existing_entry(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(md) if md.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Creates a file symlink at `link` pointing at the relative `target`.
#[cfg(unix)]
fn create_symlink(target: &str, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Creates a file symlink at `link` pointing at the relative `target`.
/// Requires developer mode or elevated privileges on Windows; failure
/// falls back to a copy in [`publish_one`] (R4).
///
/// The target's separators are converted to `\`: Windows stores reparse
/// points verbatim and does **not** translate `/` in relative targets —
/// links created with forward-slash targets exist but fail to resolve
/// with ERROR_INVALID_NAME (os error 123), which is why symlinks are
/// also disabled by default on Windows (see `cli::hf_cache_cmd::symlinks_enabled`).
#[cfg(windows)]
fn create_symlink(target: &str, link: &Path) -> io::Result<()> {
    let windows_target = target.replace('/', "\\");
    std::os::windows::fs::symlink_file(&windows_target, link)
}

/// No symlink support to attempt on other platforms — [`publish_one`]
/// copies instead (R4).
#[cfg(not(any(unix, windows)))]
fn create_symlink(_target: &str, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symlinks are not supported on this platform",
    ))
}

/// Best-effort hub-style per-file publish lock,
/// `.locks/<repo-dir-name>/<repo_path with '/' → '_'>.lock` under the
/// cache root (§5.2 step 6): a coexistence gesture toward a concurrently
/// running `hf download`. Every failure to acquire yields `None` — the
/// staging + rename protocol (R5) is what actually protects the cache.
/// Dropping the guard deletes the lock file. The exact hub naming is
/// verified in M4 (§11.2).
struct HubFileLock {
    path: PathBuf,
}

impl HubFileLock {
    fn acquire(repo_dir: &Path, repo_path: &str) -> Option<Self> {
        let repo_name = repo_dir.file_name()?.to_str()?;
        let file = format!("{}.lock", repo_path.replace('/', "_"));
        let path = repo_dir
            .parent()?
            .join(LOCKS_DIR)
            .join(repo_name)
            .join(file);
        fs::create_dir_all(path.parent()?).ok()?;
        let mut lock = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .ok()?;
        let _ = writeln!(lock, "{}", std::process::id());
        Some(Self { path })
    }
}

impl Drop for HubFileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// refs (R2)
// ---------------------------------------------------------------------------

/// Writes `refs/<ref_name>` containing the resolved commit SHA — raw, no
/// trailing newline, byte-identical to hub writes (R2). `ref_name` `None`
/// (a raw-SHA revision request) is a no-op: hub writes refs only for
/// branch/tag revisions. Nested ref names (e.g. `release/v2`) create their
/// parent directories.
pub fn write_refs(repo_dir: &Path, ref_name: Option<&str>, sha: &str) -> io::Result<()> {
    let Some(ref_name) = ref_name else {
        return Ok(());
    };
    validate_relative_path(ref_name)?;
    let path = repo_dir.join(REFS_DIR).join(ref_name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, sha)
}

// ---------------------------------------------------------------------------
// Sync lock (§5.2 step 6)
// ---------------------------------------------------------------------------

/// Guard for the per-repo sync lock; dropping it deletes the lock file.
/// A process that dies between acquire and drop leaves the file behind for
/// the 24h staleness policy ([`sync_lock_is_stale`]) to reclaim.
#[derive(Debug)]
pub struct SyncLockGuard {
    path: PathBuf,
}

/// Acquires `<staging>/.sync.lock` with an exclusive create (§5.2 step 6),
/// serializing concurrent `hf-cache sync` runs on the same repo. An
/// existing lock older than 24h is considered abandoned and stolen with a
/// warning; a fresh one fails with [`io::ErrorKind::AlreadyExists`]. The
/// lock file records `<unix-seconds> pid=<pid>`.
pub fn acquire_sync_lock(staging: &Path) -> io::Result<SyncLockGuard> {
    fs::create_dir_all(staging)?;
    let path = staging.join(SYNC_LOCK_FILE);
    let now = SystemTime::now();
    match create_sync_lock(&path, now) {
        Ok(guard) => Ok(guard),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let age = sync_lock_age(&path, now);
            if sync_lock_is_stale(age) {
                eprintln!(
                    "warning: stealing stale sync lock at {} (age {}s exceeds {}s)",
                    path.display(),
                    age.as_secs(),
                    STALE_SYNC_LOCK.as_secs()
                );
                fs::remove_file(&path)?;
                create_sync_lock(&path, SystemTime::now())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "another hf-cache sync appears to be running (lock at {} is {}s old)",
                        path.display(),
                        age.as_secs()
                    ),
                ))
            }
        }
        Err(e) => Err(e),
    }
}

/// Creates the lock file exclusively and records pid + timestamp.
fn create_sync_lock(path: &Path, now: SystemTime) -> io::Result<SyncLockGuard> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    writeln!(file, "{secs} pid={}", std::process::id())?;
    Ok(SyncLockGuard {
        path: path.to_path_buf(),
    })
}

/// Age of an existing lock file: the timestamp recorded inside it when
/// readable, else the file's mtime; zero (treated as fresh) when neither
/// is available, so a corrupt lock never invites an eager steal.
fn sync_lock_age(path: &Path, now: SystemTime) -> Duration {
    let recorded = fs::read_to_string(path)
        .ok()
        .and_then(|content| {
            content
                .split_whitespace()
                .next()
                .and_then(|token| token.parse::<u64>().ok())
        })
        .map(|secs| UNIX_EPOCH + Duration::from_secs(secs));
    let anchor = recorded
        .or_else(|| fs::metadata(path).and_then(|m| m.modified()).ok())
        .unwrap_or(now);
    now.duration_since(anchor).unwrap_or_default()
}

/// Pure staleness rule for sync locks: older than 24h ⇒ stealable on the
/// next acquire (§5.2 step 6; policy observed before freezing, §11.4).
/// Kept pure — taking the age explicitly — so the threshold is
/// unit-testable without touching real lock files.
pub fn sync_lock_is_stale(age: Duration) -> bool {
    age > STALE_SYNC_LOCK
}

impl Drop for SyncLockGuard {
    fn drop(&mut self) {
        // Best effort: a failed delete (NFS hiccup, crash of the remover)
        // leaves the file for the 24h staleness policy to reclaim.
        let _ = fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Staging cleanup (§5.2 step 10)
// ---------------------------------------------------------------------------

/// Best-effort removal of staging remnants of published files (§5.2
/// step 10): each staged file that still exists is deleted (the normal
/// publish path already renamed it away; this covers copy-mode leftovers
/// and crashed runs), then now-empty parent directories are pruned up to —
/// never including — the staging root. `.incomplete` files of *failed*
/// downloads survive: they carry resume value. Individual failures are
/// ignored; the call always succeeds.
pub fn cleanup_staging(repo_dir: &Path, published: &[String]) -> io::Result<()> {
    let staging = staging_dir(repo_dir);
    for repo_path in published {
        if validate_relative_path(repo_path).is_err() {
            continue; // never the output of our own pipeline; skip defensively
        }
        let staged = staging.join(repo_path);
        let _ = fs::remove_file(&staged);
        let mut dir = staged.parent().map(Path::to_path_buf);
        while let Some(current) = dir {
            if current == staging {
                break;
            }
            // remove_dir only succeeds on empty dirs: non-empty (or
            // already gone) stops the prune walk.
            if fs::remove_dir(&current).is_err() {
                break;
            }
            dir = current.parent().map(Path::to_path_buf);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared guards
// ---------------------------------------------------------------------------

/// Rejects repo-relative paths and ref names that could escape their
/// anchor directory: absolute paths, `..`, non-plain components, and
/// backslashes (a Windows separator — rejected for cross-platform layout
/// determinism, E1 in §9). The engine's staging validation already covers
/// downloads; this guards the snapshot and refs paths this module creates
/// itself.
///
/// Deliberately stricter than `paths::sanitize` (backslashes rejected
/// outright, plain components only) because hub-cache layout paths must be
/// identical on every platform; shared per-component sanitization concepts
/// live in `paths::sanitize` where they overlap.
fn validate_relative_path(path: &str) -> io::Result<()> {
    if path.is_empty() {
        return Err(invalid_input("path must not be empty".to_string()));
    }
    let parsed = Path::new(path);
    if !parsed.is_relative() {
        return Err(invalid_input(format!("path must be relative: {path}")));
    }
    for component in parsed.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(invalid_input(format!(
                "path must consist of plain name components: {path}"
            )));
        }
    }
    if path.contains('\\') {
        return Err(invalid_input(format!(
            "path must not contain backslashes: {path}"
        )));
    }
    Ok(())
}

/// Shorthand for an [`io::ErrorKind::InvalidInput`] error with a message.
fn invalid_input(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LfsInfo;

    /// Unique-per-test temp dir, following the repo convention
    /// (`std::env::temp_dir` + tag + pid, like `paths.rs`/`engine.rs`).
    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rhd-hf-cache-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// Tree-entry builder: path, sibling size, tree oid, LFS oid.
    fn repo_file(
        path: &str,
        size: Option<u64>,
        oid: Option<&str>,
        lfs_oid: Option<(&str, u64)>,
    ) -> RepoFile {
        RepoFile {
            rfilename: path.to_string(),
            size,
            oid: oid.map(str::to_string),
            lfs: lfs_oid.map(|(oid, size)| LfsInfo {
                oid: oid.to_string(),
                size,
                pointer_size: 134,
            }),
        }
    }

    const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const GIT_OID: &str = "d6a7702e2c35b4b1f9c8e3e9c2b1a0d4f7e6c5b4";
    const LFS_OID: &str = "6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b";

    // ---- pure layout math ----

    #[test]
    fn repo_dir_names_follow_hub_convention() {
        assert_eq!(repo_dir_name("a/b"), "models--a--b");
        assert_eq!(
            repo_dir_name("Qwen/Qwen2.5-7B-Instruct"),
            "models--Qwen--Qwen2.5-7B-Instruct"
        );
        assert_eq!(repo_dir_name("gpt2"), "models--gpt2");
    }

    #[test]
    fn git_blob_sha1_matches_known_git_vectors() {
        let dir = tmp("sha1-vectors");
        let empty = dir.join("empty");
        fs::write(&empty, b"").expect("write empty file");
        // `git hash-object` of the empty file.
        assert_eq!(
            git_blob_sha1(&empty).unwrap(),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
        let hello = dir.join("hello");
        fs::write(&hello, b"hello\n").expect("write hello file");
        assert_eq!(
            git_blob_sha1(&hello).unwrap(),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_blob_sha1_streams_files_larger_than_the_buffer() {
        // 1 MiB + 1 B of deterministic bytes: multiple 64 KiB chunks plus a
        // short tail. The streaming result must equal the one-shot
        // header ++ content hash.
        let dir = tmp("sha1-big");
        let path = dir.join("big.bin");
        let mut state = 0x9E3779B97F4A7C15u64;
        let mut payload = Vec::with_capacity(1024 * 1024 + 1);
        while payload.len() < payload.capacity() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            payload.extend_from_slice(&state.to_le_bytes());
        }
        fs::write(&path, &payload).expect("write big file");

        let mut expected = Sha1::new();
        expected.update(format!("blob {}\0", payload.len()).as_bytes());
        expected.update(&payload);

        assert_eq!(
            git_blob_sha1(&path).unwrap(),
            hex::encode(expected.finalize())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn git_blob_sha1_errors_when_the_file_changes_size_underneath() {
        // Plan amendment (W1.6): stat length vs bytes read mismatch errors
        // instead of hashing a torn file. A FIFO reports stat length 0 but
        // yields real bytes, which triggers the check deterministically.
        let dir = tmp("sha1-race");
        let fifo = dir.join("pipe");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo available on unix CI")
            .success());

        let writer = std::thread::spawn({
            let fifo = fifo.clone();
            move || {
                use std::io::Write;
                if let Ok(mut f) = fs::OpenOptions::new().write(true).open(&fifo) {
                    let _ = f.write_all(b"hello\n");
                }
            }
        });

        let err = git_blob_sha1(&fifo).expect_err("stat/read mismatch must error");
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(err.to_string().contains("changed size while hashing"));
        writer.join().expect("writer thread");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_and_staging_helpers_match_the_layout() {
        assert_eq!(
            snapshot_dir(Path::new("/cache"), "a/b", COMMIT_SHA),
            PathBuf::from("/cache/models--a--b/snapshots/").join(COMMIT_SHA)
        );
        assert_eq!(
            snapshot_symlink_target(LFS_OID, "model.safetensors"),
            format!("../../blobs/{LFS_OID}")
        );
        // Depth-aware: one more ../ per repo-path directory component (R3).
        assert_eq!(
            snapshot_symlink_target(LFS_OID, "text_encoder/a/model.safetensors"),
            format!("../../../../blobs/{LFS_OID}")
        );
        assert_eq!(
            staging_dir(Path::new("/cache/models--a--b")),
            PathBuf::from("/cache/models--a--b/.rhd-staging")
        );
    }

    // ---- plan (R6) ----

    #[test]
    fn plan_fetches_missing_blobs_with_tree_derived_oids() {
        let cache = tmp("plan-fetch");
        let tree = vec![
            repo_file(
                "model.safetensors",
                Some(100),
                Some(LFS_OID),
                Some((LFS_OID, 100)),
            ),
            repo_file("config.json", Some(10), Some(GIT_OID), None),
        ];
        let selected = vec!["model.safetensors".to_string(), "config.json".to_string()];
        let result = plan(&cache, "a/b", &tree, &selected, COMMIT_SHA, false).unwrap();
        assert_eq!(result.sha, COMMIT_SHA);
        assert!(result.up_to_date.is_empty());
        assert_eq!(result.fetch.len(), 2);
        // LFS entry: blob oid and sha256 both come from lfs.oid.
        let lfs_item = result
            .fetch
            .iter()
            .find(|i| i.repo_path == "model.safetensors")
            .unwrap();
        assert_eq!(lfs_item.blob_oid.as_deref(), Some(LFS_OID));
        assert_eq!(lfs_item.size, 100);
        assert_eq!(lfs_item.sha256.as_deref(), Some(LFS_OID));
        // Non-LFS entry: tree oid, nothing to verify.
        let plain_item = result
            .fetch
            .iter()
            .find(|i| i.repo_path == "config.json")
            .unwrap();
        assert_eq!(plain_item.blob_oid.as_deref(), Some(GIT_OID));
        assert_eq!(plain_item.size, 10);
        assert_eq!(plain_item.sha256, None);
        let _ = fs::remove_dir_all(&cache);
    }

    #[test]
    fn plan_lfs_oid_wins_over_disagreeing_tree_oid() {
        let cache = tmp("plan-lfs-precedence");
        let tree = vec![repo_file(
            "x.bin",
            Some(5),
            Some(GIT_OID),
            Some((LFS_OID, 5)),
        )];
        let selected = vec!["x.bin".to_string()];
        let result = plan(&cache, "a/b", &tree, &selected, COMMIT_SHA, false).unwrap();
        assert_eq!(result.fetch[0].blob_oid.as_deref(), Some(LFS_OID));
        let _ = fs::remove_dir_all(&cache);
    }

    #[test]
    fn plan_skips_present_size_matching_blobs() {
        let cache = tmp("plan-idempotent");
        let blobs = cache.join("models--a--b").join("blobs");
        fs::create_dir_all(&blobs).expect("create blobs dir");
        fs::write(blobs.join(LFS_OID), vec![0u8; 100]).expect("write blob");
        let tree = vec![repo_file(
            "model.safetensors",
            Some(100),
            Some(LFS_OID),
            Some((LFS_OID, 100)),
        )];
        let selected = vec!["model.safetensors".to_string()];
        let result = plan(&cache, "a/b", &tree, &selected, COMMIT_SHA, false).unwrap();
        assert!(result.fetch.is_empty());
        assert_eq!(result.up_to_date, selected);
        let _ = fs::remove_dir_all(&cache);
    }

    #[test]
    fn plan_refetches_on_size_mismatch_and_on_force() {
        let cache = tmp("plan-refetch");
        let blobs = cache.join("models--a--b").join("blobs");
        fs::create_dir_all(&blobs).expect("create blobs dir");
        fs::write(blobs.join(LFS_OID), vec![0u8; 99]).expect("write short blob");
        let tree = vec![repo_file(
            "model.safetensors",
            Some(100),
            Some(LFS_OID),
            Some((LFS_OID, 100)),
        )];
        let selected = vec!["model.safetensors".to_string()];
        // Size mismatch → re-fetch.
        let mismatched = plan(&cache, "a/b", &tree, &selected, COMMIT_SHA, false).unwrap();
        assert_eq!(mismatched.fetch.len(), 1);
        assert!(mismatched.up_to_date.is_empty());
        // Size now matches, but --force → re-fetch anyway (§5.3).
        fs::write(blobs.join(LFS_OID), vec![0u8; 100]).expect("fix blob size");
        let forced = plan(&cache, "a/b", &tree, &selected, COMMIT_SHA, true).unwrap();
        assert_eq!(forced.fetch.len(), 1);
        assert!(forced.up_to_date.is_empty());
        let _ = fs::remove_dir_all(&cache);
    }

    #[test]
    fn plan_refetches_when_tree_size_is_unknown() {
        let cache = tmp("plan-unknown-size");
        let blobs = cache.join("models--a--b").join("blobs");
        fs::create_dir_all(&blobs).expect("create blobs dir");
        fs::write(blobs.join(LFS_OID), vec![0u8; 100]).expect("write blob");
        // No sibling size and no lfs block → size unknown → not provably
        // current → conservative re-fetch.
        let tree = vec![repo_file("model.safetensors", None, Some(LFS_OID), None)];
        let selected = vec!["model.safetensors".to_string()];
        let result = plan(&cache, "a/b", &tree, &selected, COMMIT_SHA, false).unwrap();
        assert_eq!(result.fetch.len(), 1);
        assert_eq!(result.fetch[0].size, 0); // unknown size carried as 0
        let _ = fs::remove_dir_all(&cache);
    }

    #[test]
    fn plan_rejects_unknown_selections_and_defers_oidless_entries() {
        let cache = tmp("plan-errors");
        let tree = vec![repo_file("config.json", Some(10), Some(GIT_OID), None)];
        // Selected path missing from the tree.
        let missing = vec!["nope.json".to_string()];
        assert!(plan(&cache, "a/b", &tree, &missing, COMMIT_SHA, false).is_err());
        // Entry without any oid: kept as a fetch whose blob name is
        // resolved at publish time from the staged content (R1 fallback) —
        // never `up_to_date`, since the blob name is unknowable here.
        let oidless = vec![repo_file("bare.txt", Some(3), None, None)];
        let selected = vec!["bare.txt".to_string()];
        let result = plan(&cache, "a/b", &oidless, &selected, COMMIT_SHA, false).unwrap();
        assert_eq!(result.fetch.len(), 1);
        assert_eq!(result.fetch[0].blob_oid, None);
        assert!(result.up_to_date.is_empty());
        // Even with a matching-size blob already present, an oidless entry
        // re-fetches (we cannot know which blob to check).
        let blobs = cache.join("models--a--b").join("blobs");
        fs::create_dir_all(&blobs).expect("create blobs");
        fs::write(blobs.join("someoid"), [0u8; 3]).expect("write blob");
        let again = plan(&cache, "a/b", &oidless, &selected, COMMIT_SHA, false).unwrap();
        assert_eq!(again.fetch.len(), 1);
        // Escaping paths are rejected before any lookup.
        let escape = vec!["../evil".to_string()];
        assert!(plan(&cache, "a/b", &tree, &escape, COMMIT_SHA, false).is_err());
        let _ = fs::remove_dir_all(&cache);
    }

    #[test]
    fn plan_rejects_conflicting_duplicate_tree_entries_but_accepts_agreeing_ones() {
        let cache = tmp("plan-duplicates");
        let selected = vec!["f.bin".to_string()];
        // E2: duplicates must agree on oid and size.
        let conflicting = vec![
            repo_file("f.bin", Some(1), Some(GIT_OID), None),
            repo_file("f.bin", Some(2), Some(LFS_OID), None),
        ];
        assert!(plan(&cache, "a/b", &conflicting, &selected, COMMIT_SHA, false).is_err());
        let agreeing = vec![
            repo_file("f.bin", Some(1), Some(GIT_OID), None),
            repo_file("f.bin", Some(1), Some(GIT_OID), None),
        ];
        let result = plan(&cache, "a/b", &agreeing, &selected, COMMIT_SHA, false).unwrap();
        assert_eq!(result.fetch.len(), 1);
        assert_eq!(result.fetch[0].blob_oid.as_deref(), Some(GIT_OID));
        let _ = fs::remove_dir_all(&cache);
    }

    // ---- publish_one (R3/R4/R5) ----

    fn staged_file(repo: &Path, repo_path: &str, bytes: &[u8]) -> PathBuf {
        let staged = staging_dir(repo).join(repo_path);
        fs::create_dir_all(staged.parent().expect("staged parent")).expect("create staging dir");
        fs::write(&staged, bytes).expect("write staged file");
        staged
    }

    fn fetch_item(repo_path: &str, blob_oid: &str) -> FetchItem {
        FetchItem {
            repo_path: repo_path.to_string(),
            blob_oid: Some(blob_oid.to_string()),
            size: 8,
            sha256: None,
        }
    }

    /// fetch_item with the oid of the *actual* staged bytes, so the
    /// publish-time sha1 gate (R1/R5) accepts it.
    fn consistent_item(staged: &Path, repo_path: &str) -> FetchItem {
        let oid = git_blob_sha1(staged).expect("hash staged bytes");
        FetchItem {
            repo_path: repo_path.to_string(),
            blob_oid: Some(oid.clone()),
            size: 8,
            sha256: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn publish_one_symlinks_nested_paths_with_relative_target() {
        let repo = tmp("publish-symlink");
        let staged = staged_file(&repo, "a/b/c.txt", b"payload\n");
        let item = consistent_item(&staged, "a/b/c.txt");
        let published_oid = publish_one(&repo, COMMIT_SHA, &item, &staged, true).expect("publish");

        // Blob holds the bytes under the content-derived oid; staging
        // entry is gone (renamed).
        assert_eq!(
            fs::read(repo.join("blobs").join(&published_oid)).unwrap(),
            b"payload\n"
        );
        assert!(!staged.exists());

        // Nested snapshot entry is a depth-aware relative symlink (R3):
        // one ../ per repo-path directory, and reading THROUGH the link
        // yields the blob bytes — the property offline vLLM depends on.
        let link = repo.join("snapshots").join(COMMIT_SHA).join("a/b/c.txt");
        let metadata = fs::symlink_metadata(&link).expect("snapshot entry");
        assert!(metadata.file_type().is_symlink());
        assert_eq!(
            fs::read_link(&link).unwrap(),
            Path::new("../../../../blobs/").join(&published_oid)
        );
        assert_eq!(fs::read(&link).unwrap(), b"payload\n");

        // Hub-style per-file lock was held and cleaned up on drop.
        let lock = repo
            .parent()
            .unwrap()
            .join(".locks")
            .join(repo.file_name().unwrap())
            .join("a_b_c.txt.lock");
        assert!(!lock.exists());
        let _ = fs::remove_dir_all(&repo);
    }

    #[cfg(unix)]
    #[test]
    fn publish_one_top_level_link_resolves_to_the_blob() {
        let repo = tmp("publish-toplevel");
        let staged = staged_file(&repo, "config.json", b"{}\n");
        let item = consistent_item(&staged, "config.json");
        let published_oid = publish_one(&repo, COMMIT_SHA, &item, &staged, true).expect("publish");
        let link = repo.join("snapshots").join(COMMIT_SHA).join("config.json");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(&link).unwrap(),
            Path::new("../../blobs/").join(&published_oid)
        );
        // For a top-level repo path the R3 target resolves: reading
        // through the link yields the blob bytes a reader (vLLM) would see.
        assert_eq!(fs::read(&link).unwrap(), b"{}\n");
        let _ = fs::remove_dir_all(&repo);
    }

    #[cfg(unix)]
    #[test]
    fn publish_one_replaces_dangling_and_stale_snapshot_entries() {
        let repo = tmp("publish-replace");
        let snapshot = repo.join("snapshots").join(COMMIT_SHA).join("c.txt");
        fs::create_dir_all(snapshot.parent().unwrap()).expect("create snapshot dir");
        std::os::unix::fs::symlink("../../blobs/nonexistent", &snapshot).expect("dangling link");

        let staged = staged_file(&repo, "c.txt", b"payload\n");
        let item = consistent_item(&staged, "c.txt");
        let oid = publish_one(&repo, COMMIT_SHA, &item, &staged, true).expect("publish");
        assert_eq!(
            fs::read_link(&snapshot).unwrap(),
            Path::new("../../blobs/").join(&oid)
        );
        assert_eq!(fs::read(&snapshot).unwrap(), b"payload\n");

        // Re-publishing (E5 dedup: same oid, second path) replaces the
        // now-valid link with an identical one and renames a fresh staged
        // file over the existing blob.
        let staged_again = staged_file(&repo, "c.txt", b"payload\n");
        publish_one(&repo, COMMIT_SHA, &item, &staged_again, true).expect("republish");
        assert!(!staged_again.exists());
        assert_eq!(fs::read(&snapshot).unwrap(), b"payload\n");
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn publish_one_copies_when_symlinks_are_disabled() {
        let repo = tmp("publish-copy");
        // A stale regular file at the snapshot path must be replaced.
        let snapshot = repo.join("snapshots").join(COMMIT_SHA).join("c.txt");
        fs::create_dir_all(snapshot.parent().unwrap()).expect("create snapshot dir");
        fs::write(&snapshot, b"stale").expect("write stale entry");

        let staged = staged_file(&repo, "c.txt", b"payload\n");
        let item = consistent_item(&staged, "c.txt");
        let oid = publish_one(&repo, COMMIT_SHA, &item, &staged, false).expect("publish");

        assert_eq!(
            fs::read(repo.join("blobs").join(&oid)).unwrap(),
            b"payload\n"
        );
        assert!(!staged.exists());
        let metadata = fs::symlink_metadata(&snapshot).expect("snapshot entry");
        assert!(metadata.is_file()); // R4: copied, not linked
        assert!(!metadata.file_type().is_symlink());
        assert_eq!(fs::read(&snapshot).unwrap(), b"payload\n");
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn publish_one_rejects_escaping_repo_paths() {
        let repo = tmp("publish-escape");
        let staged = staged_file(&repo, "c.txt", b"payload\n");
        let item = fetch_item("../escape.txt", "cafebabe");
        assert!(publish_one(&repo, COMMIT_SHA, &item, &staged, true).is_err());
        assert!(!repo.join("blobs").exists());
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn publish_one_rejects_content_not_matching_tree_oid() {
        // R1/R5: a non-LFS file whose staged bytes do not hash to the
        // tree's git oid never enters blobs/ — the staged copy survives
        // for inspection.
        let repo = tmp("publish-mismatch");
        let staged = staged_file(&repo, "config.json", b"tampered\n");
        let wrong = git_blob_sha1(staged_file(&repo, "other", b"good bytes\n").as_path())
            .expect("hash other bytes");
        let item = fetch_item("config.json", &wrong);
        let err = publish_one(&repo, COMMIT_SHA, &item, &staged, true)
            .expect_err("mismatching content must be rejected");
        assert!(err.to_string().contains("does not match tree oid"));
        assert!(!repo.join("blobs").join(&wrong).exists());
        assert!(staged.exists(), "staged copy survives the rejection");
        let _ = fs::remove_dir_all(&repo);
    }

    #[cfg(unix)]
    #[test]
    fn publish_one_resolves_oidless_items_from_staged_content() {
        // R1 fallback: tree carried no oid — publish computes the git blob
        // sha1 from the staged bytes and returns it.
        let repo = tmp("publish-oidless");
        let staged = staged_file(&repo, "bare.txt", b"hello\n");
        let item = FetchItem {
            repo_path: "bare.txt".to_string(),
            blob_oid: None,
            size: 6,
            sha256: None,
        };
        let oid = publish_one(&repo, COMMIT_SHA, &item, &staged, true).expect("publish");
        assert_eq!(oid, "ce013625030ba8dba906f756967f9e9ca394464a");
        let link = repo.join("snapshots").join(COMMIT_SHA).join("bare.txt");
        assert_eq!(fs::read(&link).unwrap(), b"hello\n");
        let _ = fs::remove_dir_all(&repo);
    }

    // ---- ensure_snapshot_entry (R6/E9 relink) ----

    #[cfg(unix)]
    #[test]
    fn ensure_snapshot_entry_creates_missing_and_fixes_wrong_links() {
        let repo = tmp("ensure-symlink");
        fs::create_dir_all(repo.join("blobs")).expect("create blobs dir");
        fs::write(repo.join("blobs").join(GIT_OID), b"payload\n").expect("write blob");
        let file = repo_file("config.json", Some(8), Some(GIT_OID), None);
        let link = repo.join("snapshots").join(COMMIT_SHA).join("config.json");

        // Missing entry → created, resolving to the blob (E9 relink).
        ensure_snapshot_entry(&repo, COMMIT_SHA, &file, true).expect("ensure missing");
        assert_eq!(fs::read(&link).unwrap(), b"payload\n");

        // Wrong-target link → replaced with the correct one.
        fs::remove_file(&link).expect("remove link");
        std::os::unix::fs::symlink("../../blobs/wrongoid", &link).expect("wrong link");
        ensure_snapshot_entry(&repo, COMMIT_SHA, &file, true).expect("ensure wrong");
        assert_eq!(
            fs::read_link(&link).unwrap(),
            Path::new("../../blobs/").join(GIT_OID)
        );

        // Correct link → cheap no-op (still correct afterwards).
        ensure_snapshot_entry(&repo, COMMIT_SHA, &file, true).expect("ensure noop");
        assert_eq!(fs::read(&link).unwrap(), b"payload\n");
        let _ = fs::remove_dir_all(&repo);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_snapshot_entry_copy_mode_assumes_regular_entries_current() {
        let repo = tmp("ensure-copy");
        fs::create_dir_all(repo.join("blobs")).expect("create blobs dir");
        fs::write(repo.join("blobs").join(GIT_OID), b"payload\n").expect("write blob");
        let file = repo_file("config.json", Some(8), Some(GIT_OID), None);
        let entry = repo.join("snapshots").join(COMMIT_SHA).join("config.json");

        // Missing entry → copied from the blob.
        ensure_snapshot_entry(&repo, COMMIT_SHA, &file, false).expect("ensure copy");
        assert_eq!(fs::read(&entry).unwrap(), b"payload\n");
        assert!(!fs::symlink_metadata(&entry)
            .unwrap()
            .file_type()
            .is_symlink());

        // An existing regular entry is assumed current and left untouched
        // (the same assumption hub makes), even when it differs from blob.
        fs::write(&entry, b"stale").expect("write stale entry");
        ensure_snapshot_entry(&repo, COMMIT_SHA, &file, false).expect("ensure again");
        assert_eq!(fs::read(&entry).unwrap(), b"stale");

        // A foreign entry type (dangling symlink) is replaced by a copy.
        fs::remove_file(&entry).expect("remove entry");
        std::os::unix::fs::symlink("../../blobs/none", &entry).expect("dangling");
        ensure_snapshot_entry(&repo, COMMIT_SHA, &file, false).expect("ensure third");
        assert_eq!(fs::read(&entry).unwrap(), b"payload\n");
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn ensure_snapshot_entry_rejects_oidless_and_escaping_entries() {
        let repo = tmp("ensure-errors");
        // No oid anywhere: nothing to point a snapshot entry at.
        let oidless = repo_file("bare.txt", Some(3), None, None);
        assert!(ensure_snapshot_entry(&repo, COMMIT_SHA, &oidless, true).is_err());
        // Escaping repo path: rejected before any filesystem effect.
        let escape = repo_file("../evil", Some(3), Some(GIT_OID), None);
        assert!(ensure_snapshot_entry(&repo, COMMIT_SHA, &escape, true).is_err());
        assert!(!repo.join("snapshots").exists());
        let _ = fs::remove_dir_all(&repo);
    }

    // ---- refs (R2) ----

    #[test]
    fn write_refs_writes_raw_sha_for_named_refs_and_nothing_for_none() {
        let dir = tmp("refs");
        let repo = dir.join("models--a--b");
        fs::create_dir_all(&repo).expect("create repo dir");
        write_refs(&repo, Some("main"), COMMIT_SHA).expect("write ref");
        // Raw SHA, byte-exact, no trailing newline (R2).
        assert_eq!(
            fs::read(repo.join("refs").join("main")).expect("read ref"),
            COMMIT_SHA.as_bytes()
        );
        // Nested ref names create their parent directories.
        write_refs(&repo, Some("release/v2"), COMMIT_SHA).expect("write nested ref");
        assert_eq!(
            fs::read_to_string(repo.join("refs").join("release").join("v2")).unwrap(),
            COMMIT_SHA
        );
        // None (raw-SHA revision request) writes nothing at all (R2).
        let bare = dir.join("models--solo");
        fs::create_dir_all(&bare).expect("create bare repo dir");
        write_refs(&bare, None, COMMIT_SHA).expect("no-op");
        assert!(!bare.join("refs").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- sync lock (§5.2 step 6) ----

    #[test]
    fn sync_lock_create_conflict_and_drop() {
        let staging = tmp("sync-lock");
        let guard = acquire_sync_lock(&staging).expect("acquire");
        let lock = staging.join(".sync.lock");
        assert!(lock.is_file());
        // Content records timestamp first, then the pid.
        let content = fs::read_to_string(&lock).expect("read lock");
        content
            .split_whitespace()
            .next()
            .and_then(|token| token.parse::<u64>().ok())
            .expect("leading unix timestamp");
        assert!(content.contains(&format!("pid={}", std::process::id())));

        // A second acquire on a fresh lock fails with AlreadyExists.
        let err = acquire_sync_lock(&staging).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);

        drop(guard);
        assert!(!lock.exists());
        // Re-acquire after the drop succeeds.
        let _again = acquire_sync_lock(&staging).expect("re-acquire");
        let _ = fs::remove_dir_all(&staging);
    }

    #[test]
    fn sync_lock_steals_locks_older_than_24h() {
        let staging = tmp("sync-lock-stale");
        let lock = staging.join(".sync.lock");
        let old_secs = (SystemTime::now() - Duration::from_secs(25 * 60 * 60))
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        fs::write(&lock, format!("{old_secs} pid=999999")).expect("write stale lock");
        let guard = acquire_sync_lock(&staging).expect("stale lock stolen");
        // The lock was rewritten with our own pid.
        let content = fs::read_to_string(&lock).expect("read lock");
        assert!(content.contains(&format!("pid={}", std::process::id())));
        drop(guard);
        assert!(!lock.exists());
        let _ = fs::remove_dir_all(&staging);
    }

    #[test]
    fn sync_lock_treats_corrupt_content_as_fresh() {
        let staging = tmp("sync-lock-corrupt");
        let lock = staging.join(".sync.lock");
        fs::write(&lock, "garbage").expect("write corrupt lock");
        // No parsable timestamp and an mtime of ~now: not stealable.
        let err = acquire_sync_lock(&staging).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        let _ = fs::remove_dir_all(&staging);
    }

    #[test]
    fn sync_lock_staleness_threshold_is_24h_exclusive() {
        assert!(!sync_lock_is_stale(Duration::ZERO));
        assert!(!sync_lock_is_stale(Duration::from_secs(24 * 60 * 60 - 1)));
        assert!(!sync_lock_is_stale(Duration::from_secs(24 * 60 * 60)));
        assert!(sync_lock_is_stale(Duration::from_secs(24 * 60 * 60 + 1)));
    }

    // ---- staging cleanup (§5.2 step 10) ----

    #[test]
    fn cleanup_staging_removes_published_files_and_prunes_empty_parents() {
        let repo = tmp("cleanup");
        let staging = staging_dir(&repo);
        fs::create_dir_all(staging.join("a").join("b")).expect("create staging tree");
        // Leftover published file (crashed run / copy-mode path).
        fs::write(staging.join("a").join("b").join("c.txt"), b"stale").expect("write leftover");
        // A failed sibling's .incomplete file must survive (resume value).
        let incomplete = staging.join("a").join("failed.bin.incomplete");
        fs::write(&incomplete, b"partial").expect("write incomplete");

        cleanup_staging(&repo, &["a/b/c.txt".to_string()]).expect("cleanup");
        assert!(!staging.join("a").join("b").join("c.txt").exists());
        assert!(!staging.join("a").join("b").exists()); // pruned: now empty
        assert!(incomplete.is_file()); // kept
        assert!(staging.join("a").is_dir()); // still holds the .incomplete
        let _ = fs::remove_dir_all(&repo);
    }

    // ---- path validation (E1) ----

    #[test]
    fn relative_path_validation_rejects_escapes() {
        assert!(validate_relative_path("config.json").is_ok());
        assert!(validate_relative_path("a/b/c.txt").is_ok());
        assert!(validate_relative_path("").is_err());
        assert!(validate_relative_path("..").is_err());
        assert!(validate_relative_path("a/../b").is_err());
        assert!(validate_relative_path("/abs/path").is_err());
        assert!(validate_relative_path("C:\\win").is_err());
        assert!(validate_relative_path("a\\b").is_err());
    }
}
