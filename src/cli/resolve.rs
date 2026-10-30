//! File resolution (pure, unit-testable): selectors, resolve errors,
//! and mapping an HF tree listing to the concrete file list.

use super::args::DownloadArgs;
use super::events::{Event, FileDto};
use crate::models::{ModelMetadata, QuantizationGroup, RepoFile};
use std::collections::HashSet;

/// A concrete file selected for download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    /// Local destination path RELATIVE to the model root — may carry a
    /// FLARE revision-dir prefix (`<revision-dir>/<repo-relpath>`,
    /// plans/flare-models.md §3.3d). The Hub-facing URL path is
    /// `remote_path` when set, else this.
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    /// The repo-relative path the request URL uses when it differs from
    /// `filename`. `None` everywhere except the FLARE frontends.
    pub remote_path: Option<String>,
}

impl From<&RepoFile> for FileSpec {
    /// The single sibling mapping (W4.9), shared by `resolve_files'`
    /// available list and hf-cache sync's `tree_file_dtos`: filename
    /// verbatim, tree size defaulting to 0 when absent, sha256 from the
    /// LFS oid only (non-LFS git blob oids are not SHA256s).
    /// `remote_path` is `None` — the ordinary download layout uses the
    /// repo path verbatim as the local path (plans/flare-models.md §3.3d None ⇒ byte-identical).
    fn from(f: &RepoFile) -> Self {
        Self {
            filename: f.rfilename.clone(),
            size_bytes: f.size.unwrap_or(0),
            sha256: f.lfs.as_ref().map(|lfs| lfs.oid.clone()),
            remote_path: None,
        }
    }
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// No selector: only valid when the repo has exactly one downloadable file.
    Default,
    Quant(String),
    Files(Vec<String>),
    All,
}

/// Derive the selector from parsed args; rejects combined selectors.
pub fn parse_selector(args: &DownloadArgs) -> Result<Selector, String> {
    let n = usize::from(args.quant.is_some())
        + usize::from(!args.file.is_empty())
        + usize::from(args.all);
    match n {
        0 => Ok(Selector::Default),
        1 => {
            if let Some(quant) = &args.quant {
                Ok(Selector::Quant(quant.clone()))
            } else if args.all {
                Ok(Selector::All)
            } else {
                Ok(Selector::Files(args.file.clone()))
            }
        }
        _ => Err("use only one of --quant, --file, or --all".to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No selector given and the repo has more than one (or zero) files.
    Ambiguous { available: Vec<FileSpec> },
    /// The requested selector matched nothing.
    NoFilesMatch {
        selector: String,
        available: Vec<FileSpec>,
    },
}

/// Selection failures across the CLI share one wire shape (W4.9): a
/// stable `code`, a one-line human `message`, and — assembled by the
/// caller — the `available` file list. Implemented by `download`'s
/// [`ResolveError`] and `hf-cache sync`'s
/// [`crate::cli::hf_cache::SyncSelectionError`];
/// [`selection_error_event`] is the single emission path (both surfaces
/// exit `EXIT_USAGE` after emitting).
pub(in crate::cli) trait SelectionError {
    /// Stable wire code (`ambiguous`, `no_files_match`,
    /// `unknown_preset`, `empty_selection`).
    fn code(&self) -> &'static str;
    /// One-line human message.
    fn message(&self) -> String;
}

impl SelectionError for ResolveError {
    fn code(&self) -> &'static str {
        match self {
            ResolveError::Ambiguous { .. } => "ambiguous",
            ResolveError::NoFilesMatch { .. } => "no_files_match",
        }
    }

    fn message(&self) -> String {
        match self {
            ResolveError::Ambiguous { available } => format!(
                "model has {} downloadable file(s); specify --quant <TYPE>, --file <PATH>, or --all",
                available.len()
            ),
            ResolveError::NoFilesMatch { selector, .. } => {
                format!("no downloadable file matches {}", selector)
            }
        }
    }
}

/// The one selection-error emission (W4.9): identical `Event::Error`
/// shape for `download` and `hf-cache sync`. The `available` payload
/// differs per surface (download: the size-filtered resolve list as
/// DTOs; sync: the full tree DTOs) and stays the caller's input so the
/// wire bytes cannot drift.
pub(in crate::cli) fn selection_error_event<E: SelectionError>(
    err: &E,
    available: Vec<FileDto>,
) -> Event {
    Event::Error {
        code: err.code().to_string(),
        message: err.message(),
        available: Some(available),
    }
}

/// Resolve which files to download. Pure function over API data — no I/O.
///
/// GGUF multipart archives are separate files with individual SHA256s (they
/// are NOT concatenated); `--quant` naturally selects all parts of that
/// quantization.
pub fn resolve_files(
    metadata: &ModelMetadata,
    quants: &[QuantizationGroup],
    selector: &Selector,
) -> Result<Vec<FileSpec>, ResolveError> {
    // All downloadable files from the recursive tree (directories filtered,
    // same rule as the TUI's repository download)
    let available: Vec<FileSpec> = metadata
        .siblings
        .iter()
        .filter(|f| f.size.is_some() && !f.rfilename.ends_with('/'))
        .map(FileSpec::from)
        .collect();

    let mut picked: Vec<FileSpec> = match selector {
        Selector::Files(names) => {
            let mut out = Vec::new();
            for name in names {
                match available.iter().find(|f| f.filename == *name) {
                    Some(file) => out.push(file.clone()),
                    None => {
                        return Err(ResolveError::NoFilesMatch {
                            selector: format!("--file {}", name),
                            available,
                        })
                    }
                }
            }
            out
        }
        Selector::Quant(quant) => {
            let mut out = Vec::new();
            // `--quant mmproj` selects every multimodal-projector group
            // (MMPROJ, MMPROJ-Q8_0, …) in one go (issue #25)
            let wants_mmproj = quant.eq_ignore_ascii_case(crate::api::MMPROJ_QUANT_TYPE);
            for group in quants {
                let matches = group.quant_type.eq_ignore_ascii_case(quant)
                    || (wants_mmproj
                        && group.quant_type.starts_with(crate::api::MMPROJ_QUANT_TYPE));
                if matches {
                    for file in &group.files {
                        out.push(FileSpec {
                            filename: file.filename.clone(),
                            size_bytes: file.size,
                            sha256: file.sha256.clone(),
                            remote_path: None,
                        });
                    }
                }
            }
            if out.is_empty() {
                return Err(ResolveError::NoFilesMatch {
                    selector: format!("--quant {}", quant),
                    available,
                });
            }
            out
        }
        Selector::All => available.clone(),
        Selector::Default => {
            if available.len() == 1 {
                available.clone()
            } else {
                return Err(ResolveError::Ambiguous { available });
            }
        }
    };

    // Dedup by filename, preserving order (repeatable --file etc.)
    let mut seen = HashSet::new();
    picked.retain(|f| seen.insert(f.filename.clone()));

    Ok(picked)
}

impl ResolveError {
    pub(super) fn available(&self) -> &Vec<FileSpec> {
        match self {
            ResolveError::Ambiguous { available }
            | ResolveError::NoFilesMatch { available, .. } => available,
        }
    }
}

// --- FLARE preset selection (plans/flare-models.md §3.3b/§3.3c) ---------

use crate::models::{FlarePackage, RepoRefs};

/// A 40-hex commit SHA — ladder step (i)'s direct-pin shape
/// (plans/flare-models.md §3.3b).
pub(super) fn is_commit_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// One member of a FLARE package's file set: the REPO-relative path (the
/// URL truth), its tree size, and the LFS oid when present
/// (plans/flare-models.md §3.3e: the oid forwards into `expected_sha256`
/// so LFS payloads are digest-verified by default; non-LFS files like
/// directory-form `config.json`/`publisher.json` carry no oid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FlareFile {
    pub relpath: String,
    pub size: u64,
    pub sha256: Option<String>,
}

fn flare_file(f: &crate::models::RepoFile) -> FlareFile {
    FlareFile {
        relpath: f.rfilename.clone(),
        size: f.size.unwrap_or(0),
        sha256: f.lfs.as_ref().map(|lfs| lfs.oid.clone()),
    }
}

/// The plans/flare-models.md §3.3c semantic package set: packed = the
/// single file; directory = `model.safetensors` + companions. The wave-1
/// assembly (`select_package`) is the SINGLE SOURCE — its companions
/// already carry `weights/**`, `deployments/**`, and the root
/// `config.json`/`publisher.json` when the revision tree has them — so
/// this mapping adds nothing (impl-gate simplification: the old
/// tree-union was a second source that could drift). The primary (packed
/// file / root `model.safetensors`) comes FIRST.
pub(super) fn flare_package_set(package: &FlarePackage) -> Vec<FlareFile> {
    match package {
        FlarePackage::Packed { file } => vec![flare_file(file)],
        FlarePackage::Directory {
            primary,
            companions,
        } => {
            let mut out = vec![flare_file(primary)];
            out.extend(companions.iter().map(flare_file));
            out
        }
    }
}

/// Map the package set into download [`FileSpec`]s with the
/// revision-scoped destination split (plans/flare-models.md §3.3d): `filename` = the LOCAL
/// `<revision-dir>/<repo-relpath>`, `remote_path` = the repo path (the
/// URL truth). `--include`/`--exclude` filter WITHIN the set, matching
/// the REPO-relative path (patterns.rs fnmatch — `*` crosses `/`).
/// An empty result is a selection error listing the unfiltered set.
pub(super) fn select_flare_files(
    package_set: &[FlareFile],
    revision_dir: &str,
    include: &[String],
    exclude: &[String],
) -> Result<Vec<FileSpec>, ResolveError> {
    let relpaths: Vec<&str> = package_set.iter().map(|f| f.relpath.as_str()).collect();
    let kept = crate::patterns::filter_paths(&relpaths, include, exclude);
    let picked: Vec<FileSpec> = package_set
        .iter()
        .filter(|f| kept.contains(&f.relpath.as_str()))
        .map(|f| FileSpec {
            filename: format!("{}/{}", revision_dir, f.relpath),
            size_bytes: f.size,
            sha256: f.sha256.clone(),
            remote_path: Some(f.relpath.clone()),
        })
        .collect();
    if picked.is_empty() {
        let selector = if include.is_empty() && exclude.is_empty() {
            "--for flare".to_string()
        } else {
            let mut s = String::from("--for flare");
            for glob in include {
                s.push_str(&format!(" --include {}", glob));
            }
            for glob in exclude {
                s.push_str(&format!(" --exclude {}", glob));
            }
            s
        };
        return Err(ResolveError::NoFilesMatch {
            selector,
            available: package_set
                .iter()
                .map(|f| FileSpec {
                    filename: format!("{}/{}", revision_dir, f.relpath),
                    size_bytes: f.size,
                    sha256: f.sha256.clone(),
                    remote_path: Some(f.relpath.clone()),
                })
                .collect(),
        });
    }
    Ok(picked)
}

/// The PREFERRED revision name for a commit (plans/flare-models.md §3.1:
/// branch name first, else tag — the directory-segment + display name).
/// `main` never wins: payload revisions exclude it.
pub(super) fn preferred_revision_name(refs: &RepoRefs, commit: &str) -> Option<String> {
    refs.branches
        .iter()
        .filter(|b| b.name != "main")
        .find(|b| b.target_commit == commit)
        .map(|b| b.name.clone())
        .or_else(|| {
            refs.tags
                .iter()
                .find(|t| t.target_commit == commit)
                .map(|t| t.name.clone())
        })
}

/// The repo's payload revision names from `/refs` (branches other than
/// `main`, then tags, aliases included) — the listing the CLI appends to
/// GPU-detection errors and to the neither-flag error
/// (plans/flare-models.md §3.3b).
pub(super) fn payload_revision_names(refs: &RepoRefs) -> Vec<String> {
    refs.branches
        .iter()
        .filter(|b| b.name != "main")
        .map(|b| b.name.clone())
        .chain(refs.tags.iter().map(|t| t.name.clone()))
        .collect()
}

// --- FLARE pre-flight guards (plans/flare-models.md §3.3d) ---------------

use crate::models::DownloadRegistry;
use std::path::{Path, PathBuf};

/// One file of the FLARE run, as the guards see it.
pub(crate) struct FlarePreflightFile {
    /// LOCAL (revision-dir-prefixed) filename — the disk truth.
    pub filename: String,
    /// Repo-relative remote path — the URL truth.
    pub remote_path: Option<String>,
    /// Expected tree size in bytes.
    pub expected_size: u64,
}

/// A plans/flare-models.md §3.3d pre-flight conflict. The `Display` strings are the human/NDJSON
/// error messages, recovery hints included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FlarePreflightError {
    /// (i) On-disk size ≠ expected tree size.
    SizeMismatch {
        relpath: String,
        expected: u64,
        actual: u64,
        directory: String,
    },
    /// (ii) Moved branch: this run's revision directory holds a file
    /// recorded at a DIFFERENT commit.
    MovedBranch {
        recorded_commit: Option<String>,
        pin: String,
        directory: String,
    },
    /// (ii-b) Moved branch, AMBIGUOUS form (impl-gate r3 P1): the
    /// revision directory holds EXISTING files recorded at MULTIPLE
    /// distinct commits at once (partial deletes can produce it) — no
    /// single owner can be named, so the recovery offers both actions.
    AmbiguousOwner {
        /// The distinct recorded pins whose files exist (`<none>` for
        /// rows without a recorded revision).
        owners: Vec<String>,
        pin: String,
        directory: String,
    },
    /// (ii-c) Unresolvable output (impl-gate r5 P1): the run's own
    /// revision-dir spelling traverses a symlink that CANNOT be
    /// resolved NOW (dangling target, ELOOP cycle). The transport's
    /// `create_dir_all` can materialize a dangling target mid-run and
    /// retarget the traversal, so the run's real destination is
    /// UNPREDICTABLE — the pre-flight refuses to guess instead of
    /// silently skipping the ownership comparison.
    UnresolvableOutput { directory: String },
    /// (iii) Single destination per package: the URL is already recorded
    /// at a different destination (ordinary/legacy entries included).
    DestinationConflict {
        url: String,
        requested: String,
        recorded: String,
    },
}

impl std::fmt::Display for FlarePreflightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlarePreflightError::SizeMismatch {
                relpath,
                expected,
                actual,
                directory,
            } => write!(
                f,
                "file {relpath} on disk is {actual} bytes but this revision expects {expected} \
                 bytes — remove {directory} (or pin --revision to the revision that matches it)"
            ),
            FlarePreflightError::MovedBranch {
                recorded_commit,
                pin,
                directory,
            } => write!(
                f,
                "branch moved: directory {} holds commit {}, run targets {} — remove the \
                 directory or pin `--revision {pin}`",
                directory,
                recorded_commit.as_deref().unwrap_or("<none>"),
                pin
            ),
            FlarePreflightError::AmbiguousOwner {
                owners,
                pin,
                directory,
            } => write!(
                f,
                "branch moved: directory {} holds files from multiple commits ({}) — delete \
                 the directory or the registry entries, then re-run (this run targets {})",
                directory,
                owners.join(", "),
                pin
            ),
            FlarePreflightError::UnresolvableOutput { directory } => write!(
                f,
                "output path {} resolves through an unresolvable symlink — use a clean, \
                 fully-resolvable --output path",
                directory
            ),
            FlarePreflightError::DestinationConflict {
                url,
                requested,
                recorded,
            } => write!(
                f,
                "this package is already recorded at `{recorded}` — reuse that copy, re-run \
                 from that output root, or delete the registry entry to re-fetch here (TUI \
                 resume-delete covers Incomplete/HashMismatch entries only; a Completed entry \
                 is removed by deleting its row from the registry TOML, which is user-editable \
                 data) [url: {url}, requested: {requested}]",
            ),
        }
    }
}

/// The requested registration destination for one file — the exact path
/// `register_pending` would record. `None` when the path is invalid (the
/// enqueue's validate-first abort owns that failure).
fn requested_destination(base: &str, model_id: &str, filename: &str) -> Option<PathBuf> {
    crate::paths::sanitize::validate_and_sanitize_path(base, model_id, filename).ok()
}

/// The on-disk IDENTITY of `p` (impl-gate r2 P1 + r3 P1 + r4 P1): an
/// INCREMENTAL resolve-at-hop walk that never lexically deletes a
/// component it has not resolved, so a base spelled `--output
/// ../work/models` still compares EQUAL to the recorded absolute row
/// for the same directory. `requested_destination` absolutizes but
/// does NOT normalize `..`/symlink components in the path it RETURNS
/// (canonicalization happens only inside its containment checks), so
/// raw string comparisons miss the same directory.
///
/// The walk: (1) absolutize; (2) anchor at the deepest EXISTING
/// ancestor, canonicalized — the OS resolves any `.`/`..`/symlink
/// components inside the anchor during canonicalize, so the anchor is
/// a canonical, symlink-free prefix; (3) walk the remaining tail one
/// component at a time: `.` skips; `..` pops the CURRENT path — safe
/// because the resolved prefix is canonical (its parent IS the true
/// parent) and anything appended since the last resolution either
/// canonicalized or never existed; every other component is joined
/// and, when it EXISTS on disk, canonicalized AT THE HOP — a symlink
/// is resolved the moment it is traversed, so a LATER `..` pops the
/// RESOLVED parent (POSIX-faithful). A component that does not exist
/// is appended lexically; a later `..` popping it is correct (it was
/// never resolved).
///
/// r4 P1 — the r3 anchor→lexical→re-anchor FIXPOINT still lost
/// symlinks: its lexical pass popped components BEFORE any re-anchor
/// pass could resolve them (`missing/../alias/../tail` deleted
/// `alias` before re-anchoring could see it — codex r4), so the
/// identity pointed at the WRONG directory and the pre-flight guards
/// compared past the collision. Resolve-at-hop removes the need for
/// any fixpoint: the walk is one pass, finite (a step per component),
/// and symlink cycles terminate because a cyclic candidate FAILS
/// canonicalize (ELOOP) and the walk refuses to guess (r5
/// fail-closed: `None`).
/// r5 P1 — FAIL-CLOSED: a component that EXISTS on disk (per
/// `symlink_metadata`, which follows nothing — a dangling symlink
/// still exists) but FAILS `canonicalize` is a symlink that cannot be
/// resolved NOW (dangling target, ELOOP cycle); its FUTURE resolution
/// is unpredictable — the transport's `create_dir_all` can materialize
/// the dangling target and retarget the traversal at transfer time
/// (codex r5: `/R/dirA/missing/../../alias/..` predicted
/// `/R/a/b/gb202`, landed on `/R/dirA/a/b/gb202`). The walk returns
/// `None` — identity unknowable — instead of retaining the lexical
/// candidate and letting a later `..` pop it (the r4 behavior GUESSED
/// a directory the run would never actually use). A component that
/// does NOT exist stays lexical: popping it later is correct — it was
/// never resolved, so that is not a guess.
/// `None` when NOTHING along `p` exists — no on-disk identity to
/// compare (for an absolutized path the filesystem root always
/// exists, so in practice `None` only when the cwd itself is
/// unresolvable for a relative `p`) — or when a traversed EXISTING
/// component cannot be resolved now (the r5 fail-closed case above).
fn anchored_fs_identity(p: &Path) -> Option<PathBuf> {
    use std::path::Component;
    // (1) Absolutize: a relative input resolves against the cwd, the
    // same base the old anchor search reached implicitly through
    // cwd-relative canonicalize.
    let absolute: PathBuf = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(p)
    };
    let components: Vec<Component<'_>> = absolute.components().collect();
    // (2) Anchor at the deepest EXISTING ancestor. The search tries
    // the whole path first (an existing path is its own nearest
    // ancestor → full canonicalization, the exact identity), then
    // walks up; an absolutized path always bottoms out at `/`.
    let mut split = components.len();
    let mut identity: Option<PathBuf> = None;
    for keep in (0..=components.len()).rev() {
        let prefix: PathBuf = components[..keep].iter().collect();
        if let Ok(canonical) = prefix.canonicalize() {
            split = keep;
            identity = Some(canonical);
            break;
        }
    }
    let mut identity = identity?;
    // (3) Walk the tail, resolving every existing component AT THE
    // HOP so a subsequent `..` pops the RESOLVED parent — never an
    // unresolved symlink.
    for component in &components[split..] {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // `identity`'s resolved prefix is canonical and every
                // appended-since component either canonicalized or
                // never existed, so the lexical parent IS the true
                // parent. At the root `parent()` is None: keep it
                // (`/..` == `/`).
                if let Some(parent) = identity.parent() {
                    identity = parent.to_path_buf();
                }
            }
            other => {
                let candidate = identity.join(other.as_os_str());
                // Resolve-at-hop with FAIL-CLOSED refusal (impl-gate
                // r5 P1): a candidate that EXISTS on disk (per
                // `symlink_metadata`, which follows nothing — a
                // dangling symlink still exists) yet FAILS
                // `canonicalize` is a symlink that cannot be resolved
                // NOW (dangling target, ELOOP cycle); its future
                // resolution is UNPREDICTABLE because the transport's
                // `create_dir_all` can materialize the dangling target
                // and retarget this traversal at transfer time. The r4
                // behavior retained the lexical candidate, so a
                // following `..` popped it and the walk GUESSED a
                // directory the run would never actually use (codex
                // r5: predicted `/R/a/b/gb202`, landed on
                // `/R/dirA/a/b/gb202`). Identity is unknowable —
                // return `None` rather than guess. A component that
                // does NOT exist stays lexical: it was never resolved,
                // so a later `..` popping it is correct (not a guess).
                match candidate.canonicalize() {
                    Ok(canonical) => identity = canonical,
                    Err(_) if candidate.symlink_metadata().is_ok() => return None,
                    Err(_) => identity = candidate,
                }
            }
        }
    }
    Some(identity)
}

/// [`anchored_fs_identity`] with the nothing-exists fallback (impl-gate
/// r2 P1): the absolutized input, unchanged — a pure-string identity for
/// paths that touch no disk, so prefix comparisons degrade to today's
/// raw behavior instead of panicking or inventing components.
fn fs_identity(p: &Path) -> PathBuf {
    anchored_fs_identity(p).unwrap_or_else(|| {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(p)
        }
    })
}

/// plans/flare-models.md §3.3d pre-flight, PURE over the on-disk registry (the caller reads it
/// with `registry::read_registry` — reads take no lock) plus targeted
/// on-disk existence/size facts. Run BEFORE enqueue; any conflict is a
/// hard error. Check order: (i) per-file size, (ii) moved-branch scoped
/// to THIS run's `<model_root>/<revision-dir>/` prefix, (iii) per-file
/// single-destination-per-package (same URL, ANY differing recorded
/// destination — full path, ordinary/legacy entries included; no on-disk
/// carve-out: deleting the ENTRY clears it, deleting the DIRECTORY does
/// not).
///
/// Every destination comparison goes through `requested_destination` —
/// the registration-identical builder (absolutize + canonicalize) — and
/// then through `fs_identity`, because the builder absolutizes but does
/// NOT normalize `..`/symlink components in the path it RETURNS: a base
/// spelled `--output ../work/models` must still match the recorded
/// absolute row for the same directory (impl-gate r1 P1 absolutize fix +
/// r2 P1 identity fix).
///
/// Verification backstop: this pre-flight is the FAST-FAIL layer, not
/// the last line of defense. When verification is enabled (the default)
/// the engine re-verifies every AlreadyExists file against the CURRENT
/// run's expected sha256 (`download::handle_existing_file` queues the
/// existing-file skip for verification — src/download/mod.rs ~:420), so
/// stale bytes a pre-flight heuristic misses are still caught downstream
/// as a hash mismatch. `--no-verify` is the user-accepted residual risk:
/// there, ONLY this pre-flight stands between the run and another
/// commit's bytes. Spellings that traverse unresolvable symlinks are
/// refused outright — the guard never predicts across a component
/// whose resolution can change under directory creation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn flare_preflight(
    registry: &DownloadRegistry,
    base: &str,
    model_id: &str,
    revision_dir: &str,
    pin: &str,
    files: &[FlarePreflightFile],
) -> Result<(), FlarePreflightError> {
    // Registration-identical per-file destinations (guard (i) fs checks and
    // the guard (ii) prefix both compare against the exact paths the
    // enqueue would record).
    let destinations: Vec<Option<PathBuf>> = files
        .iter()
        .map(|file| requested_destination(base, model_id, &file.filename))
        .collect();

    // (i) on-disk size vs expected tree size per file.
    for (file, final_path) in files.iter().zip(&destinations) {
        let Some(final_path) = final_path else {
            continue; // the enqueue's validate-first abort owns this
        };
        if let Ok(meta) = std::fs::metadata(final_path) {
            let actual = meta.len();
            if actual != file.expected_size {
                return Err(FlarePreflightError::SizeMismatch {
                    relpath: file
                        .remote_path
                        .clone()
                        .unwrap_or_else(|| file.filename.clone()),
                    expected: file.expected_size,
                    actual,
                    directory: final_path
                        .parent()
                        .unwrap_or(Path::new(""))
                        .display()
                        .to_string(),
                });
            }
        }
    }

    // (ii) moved-branch guard, scoped to THIS run's revision directory —
    // the registration-identical prefix (a probe file under the revision
    // dir; its parent is the dir itself), compared by on-disk IDENTITY on
    // BOTH sides (`anchored_fs_identity` on the probe-derived dir —
    // hard-erroring when the run's own spelling is unresolvable, r5 —
    // and `fs_identity` on each `local_path`): the raw strings would
    // miss whenever the base is spelled through `..`/symlinks (impl-gate
    // r2 P1).
    // On-disk existence is what makes the recovery honest: deleting the
    // directory clears the guard (stale registry entries for vanished
    // files are ignored). Directory ownership is EXISTENCE-BASED and
    // ORDER-INDEPENDENT (impl-gate r3 P1): among this model's rows under
    // the dir, only rows whose file still EXISTS vote, and the OWNER SET
    // is those rows's distinct revision pins. The registry Vec order
    // carries NO ownership evidence — `upsert_pending` preserves an
    // already-recorded URL in place, so the lifecycle
    // C → delete → B → delete → C leaves the Vec order [C, B] while C's
    // bytes are the ones on disk (the r2 last-row-wins predicate crowned
    // B, the last ROW, and let a B re-run silently reuse C's bytes).
    // Owner set → verdict: EMPTY = fresh or fully-deleted dir (pass);
    // exactly {pin} = the bytes on disk are the current commit's,
    // idempotent re-run (pass — supersedes any vanished rows); any other
    // SINGLE pin = moved branch (error naming the owner whose bytes are
    // on disk); MORE than one distinct pin with existing files =
    // ambiguous residue from partial deletes (error directing the user
    // to delete the directory or the registry entries).
    let run_dir = requested_destination(base, model_id, &format!("{revision_dir}/_"))
        .and_then(|probe| probe.parent().map(Path::to_path_buf));
    if let Some(run_dir) = run_dir {
        // The RUN's own revision-dir identity must be KNOWABLE
        // (impl-gate r5 P1): `anchored_fs_identity` returns `None`
        // when the spelling traverses a symlink that cannot be
        // resolved NOW (dangling target, ELOOP cycle) — the
        // transport's `create_dir_all` can materialize the dangling
        // target mid-run, making the run's real destination
        // UNPREDICTABLE (codex r5: `/R/dirA/missing/../../alias/..`
        // predicted `/R/a/b/gb202`, landed on `/R/dirA/a/b/gb202` —
        // another commit's files, with equal sizes and different
        // URLs no other guard separates). The old path silently
        // skipped the ownership comparison (`fs_identity` degraded
        // to the lexical spelling, which misses every recorded row);
        // now the run is REFUSED outright. Entry-row identities (the
        // OTHER side) may still degrade to lexical — only the RUN's
        // own spelling must be clean.
        let Some(dir) = anchored_fs_identity(&run_dir) else {
            return Err(FlarePreflightError::UnresolvableOutput {
                directory: run_dir.display().to_string(),
            });
        };
        let mut owners: Vec<Option<&str>> = Vec::new();
        for entry in &registry.downloads {
            if entry.model_id != model_id {
                continue;
            }
            if !fs_identity(Path::new(&entry.local_path)).starts_with(&dir) {
                continue;
            }
            if !Path::new(&entry.local_path).exists() {
                continue; // vanished file: the stale entry does not block
            }
            let owner = entry.revision.as_deref();
            if !owners.contains(&owner) {
                owners.push(owner);
            }
        }
        match owners.as_slice() {
            [] => {}                           // fresh or fully-deleted dir — no ownership conflict
            [only] if *only == Some(pin) => {} // idempotent re-run
            [only] => {
                return Err(FlarePreflightError::MovedBranch {
                    recorded_commit: only.map(str::to_string),
                    pin: pin.to_string(),
                    directory: dir.display().to_string(),
                });
            }
            many => {
                return Err(FlarePreflightError::AmbiguousOwner {
                    owners: many
                        .iter()
                        .map(|owner| owner.unwrap_or("<none>").to_string())
                        .collect(),
                    pin: pin.to_string(),
                    directory: dir.display().to_string(),
                });
            }
        }
    }

    // (iii) single-destination-per-package: one URL must mean one
    // recorded destination. `path_matches` first (raw string equality,
    // or both files existing and canonically equal); when that misses,
    // compare the on-disk IDENTITIES of both sides — `fs_identity`
    // anchors each path at its nearest existing ancestor, so a
    // `..`/symlink-spelled base whose FILE does not yet exist (full
    // canonicalize fails) still proves same-destination instead of
    // raising a false conflict. When either side touches NOTHING on
    // disk (no identity to compare), the raw comparison above decides.
    for (file, requested) in files.iter().zip(&destinations) {
        let url = crate::api::resolve_url(
            model_id,
            file.remote_path.as_deref().unwrap_or(&file.filename),
            pin,
        );
        let Some(requested) = requested else {
            continue;
        };
        let requested_identity = anchored_fs_identity(requested);
        for entry in &registry.downloads {
            if entry.url != url {
                continue;
            }
            if crate::registry::path_matches(&entry.local_path, requested) {
                continue; // the same destination — idempotent re-run
            }
            if let Some(recorded_identity) = anchored_fs_identity(Path::new(&entry.local_path)) {
                if Some(&recorded_identity) == requested_identity.as_ref() {
                    continue; // same destination on disk, differently spelled
                }
            }
            return Err(FlarePreflightError::DestinationConflict {
                url,
                requested: requested.display().to_string(),
                recorded: entry.local_path.clone(),
            });
        }
    }

    Ok(())
}
