//! File resolution (pure, unit-testable): selectors, resolve errors,
//! and mapping an HF tree listing to the concrete file list.

use super::args::DownloadArgs;
use super::events::{Event, FileDto};
use crate::models::{ModelMetadata, QuantizationGroup, RepoFile};
use std::collections::HashSet;

/// A concrete file selected for download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
}

impl From<&RepoFile> for FileSpec {
    /// The single sibling mapping (W4.9), shared by `resolve_files'`
    /// available list and hf-cache sync's `tree_file_dtos`: filename
    /// verbatim, tree size defaulting to 0 when absent, sha256 from the
    /// LFS oid only (non-LFS git blob oids are not SHA256s).
    fn from(f: &RepoFile) -> Self {
        Self {
            filename: f.rfilename.clone(),
            size_bytes: f.size.unwrap_or(0),
            sha256: f.lfs.as_ref().map(|lfs| lfs.oid.clone()),
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
