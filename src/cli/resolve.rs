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

/// The plans/flare-models.md §3.3c semantic package set — wave-1's `choose_file` rule plus the
/// two root metadata files: packed = the single file; directory =
/// `model.safetensors` + `weights/**` + `deployments/**` companions +
/// `config.json` + `publisher.json` when present in the revision's tree
/// (the two root files are the directory form's — a packed package is
/// self-contained). The primary (packed file / root `model.safetensors`)
/// comes FIRST.
pub(super) fn flare_package_set(
    package: &FlarePackage,
    tree: &[crate::models::RepoFile],
) -> Vec<FlareFile> {
    match package {
        FlarePackage::Packed { file } => vec![flare_file(file)],
        FlarePackage::Directory {
            primary,
            companions,
        } => {
            let mut out = vec![flare_file(primary)];
            out.extend(companions.iter().map(flare_file));
            // Root metadata files (plain git blobs — no LFS oid): they ride
            // the package so the served form is complete.
            for name in ["config.json", "publisher.json"] {
                if let Some(found) = tree.iter().find(|f| f.rfilename == name) {
                    out.push(flare_file(found));
                }
            }
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
pub(super) struct FlarePreflightFile {
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
pub(super) enum FlarePreflightError {
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

/// plans/flare-models.md §3.3d pre-flight, PURE over the on-disk registry (the caller reads it
/// with `registry::read_registry` — reads take no lock) plus targeted
/// on-disk existence/size facts. Run BEFORE enqueue; any conflict is a
/// hard error. Check order: (i) per-file size, (ii) moved-branch scoped
/// to THIS run's `<model_root>/<revision-dir>/` prefix, (iii) per-file
/// single-destination-per-package (same URL, ANY differing recorded
/// destination — full path, ordinary/legacy entries included; no on-disk
/// carve-out: deleting the ENTRY clears it, deleting the DIRECTORY does
/// not).
#[allow(clippy::too_many_arguments)]
pub(super) fn flare_preflight(
    registry: &DownloadRegistry,
    base: &str,
    model_id: &str,
    model_root: &Path,
    revision_dir: &str,
    pin: &str,
    files: &[FlarePreflightFile],
) -> Result<(), FlarePreflightError> {
    // (i) on-disk size vs expected tree size per file.
    for file in files {
        let Some(final_path) = crate::download::final_path_for(model_root, &file.filename) else {
            continue; // the enqueue's validate-first abort owns this
        };
        if let Ok(meta) = std::fs::metadata(&final_path) {
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

    // (ii) moved-branch guard, scoped to THIS run's revision directory.
    // On-disk existence is what makes the recovery honest: deleting the
    // directory clears the guard (stale registry entries for vanished
    // files are ignored).
    let dir_prefix = format!(
        "{}/",
        model_root
            .join(revision_dir)
            .to_string_lossy()
            .replace('\\', "/")
    );
    for entry in &registry.downloads {
        if entry.model_id != model_id {
            continue;
        }
        let local = entry.local_path.replace('\\', "/");
        if !local.starts_with(&dir_prefix) {
            continue;
        }
        if entry.revision.as_deref() == Some(pin) {
            continue; // same commit — the idempotent re-run case
        }
        if !Path::new(&entry.local_path).exists() {
            continue; // vanished file: the stale entry does not block
        }
        return Err(FlarePreflightError::MovedBranch {
            recorded_commit: entry.revision.clone(),
            pin: pin.to_string(),
            directory: model_root.join(revision_dir).display().to_string(),
        });
    }

    // (iii) single-destination-per-package: one URL must mean one
    // recorded destination.
    for file in files {
        let url = crate::api::resolve_url(
            model_id,
            file.remote_path.as_deref().unwrap_or(&file.filename),
            pin,
        );
        let Some(requested) = requested_destination(base, model_id, &file.filename) else {
            continue;
        };
        for entry in &registry.downloads {
            if entry.url != url {
                continue;
            }
            if crate::registry::path_matches(&entry.local_path, &requested) {
                continue; // the same destination — idempotent re-run
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
