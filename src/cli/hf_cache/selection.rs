//! Pure file-selection logic for `hf-cache sync` (§2.2 precedence, §2.3
//! preset tables): no I/O, no network, no cache access — the whole
//! selector is unit-testable through [`select_sync_files`].
//!
//! Split out of `cli/hf_cache_cmd.rs` (plan W3.5); moved verbatim. The
//! [`crate::hf_cache`] plan/publish pipeline consumes the selected paths in
//! [`super::sync`].

use crate::cli::events::FileDto;
use crate::cli::resolve::{FileSpec, SelectionError};
use crate::models::ModelMetadata;

/// How a sync's file selection was derived (§2.2) — `WholeRepo` triggers
/// the `--for vllm` tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// Explicit positional `FILE…` — exactly those files.
    Files,
    /// `--include`/`--exclude` globs over the full tree.
    Patterns,
    /// A `--for <PRESET>` allow/ignore table (§2.3).
    Preset,
    /// No selector: the whole repository (hf `download` parity).
    WholeRepo,
}

/// Selection failures (§2.2): all map to [`EXIT_USAGE`] with the full
/// structured file list attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncSelectionError {
    /// A positional `FILE` is not present in the repository tree.
    MissingPositional {
        path: String,
        available: Vec<String>,
    },
    /// `--for` named a preset this binary does not know (unreachable via
    /// clap's `parse_preset`; kept for direct callers).
    UnknownPreset { name: String },
    /// Every mode plus `--exclude` filtering left nothing to sync.
    EmptySelection { available: Vec<String> },
}

impl SelectionError for SyncSelectionError {
    // The impl is `pub(in crate::cli)`-visible through the trait; the
    // trait itself lives in `cli/resolve.rs` next to `ResolveError` so
    // both selection vocabularies share the one emission path
    // (`resolve::selection_error_event`, W4.9).
    fn code(&self) -> &'static str {
        match self {
            SyncSelectionError::MissingPositional { .. } => "no_files_match",
            SyncSelectionError::UnknownPreset { .. } => "unknown_preset",
            SyncSelectionError::EmptySelection { .. } => "empty_selection",
        }
    }

    fn message(&self) -> String {
        match self {
            SyncSelectionError::MissingPositional { path, .. } => {
                format!("file not present in repository: {path}")
            }
            SyncSelectionError::UnknownPreset { name } => {
                format!("unknown preset {name:?} — available presets: vllm")
            }
            SyncSelectionError::EmptySelection { .. } => {
                "selection matched no files in the repository".to_string()
            }
        }
    }
}

/// Resolve the files a sync targets, per the §2.2 precedence:
///
/// 1. positional `FILE…` → exactly those files (each must exist in the
///    tree; duplicates collapse, order preserved),
/// 2. else `--include`/`--exclude` → Python-fnmatch globs over the full
///    tree (`*` crosses `/`, §7),
/// 3. else `--for vllm` → the preset allow/ignore table from §2.3
///    ([`crate::patterns::VLLM_ALLOW`]/[`crate::patterns::VLLM_IGNORE`]),
/// 4. else the whole repository ([`SelectionMode::WholeRepo`] — the
///    caller prints the `--for vllm` tip).
///
/// `--exclude` applies on top of every mode (§2.2). An empty selection
/// after all filtering is an error carrying the available file list.
pub fn select_sync_files(
    tree: &[&str],
    files: &[String],
    include: &[String],
    exclude: &[String],
    preset: Option<&str>,
) -> Result<(Vec<String>, SelectionMode), SyncSelectionError> {
    let available = || {
        tree.iter()
            .map(|path| (*path).to_string())
            .collect::<Vec<_>>()
    };
    let (mut selected, mode) = if !files.is_empty() {
        let mut picked: Vec<String> = Vec::with_capacity(files.len());
        for file in files {
            if !tree.contains(&file.as_str()) {
                return Err(SyncSelectionError::MissingPositional {
                    path: file.clone(),
                    available: available(),
                });
            }
            if !picked.contains(file) {
                picked.push(file.clone());
            }
        }
        (picked, SelectionMode::Files)
    } else if !include.is_empty() {
        let picked = crate::patterns::filter_paths(tree, include, exclude)
            .into_iter()
            .map(String::from)
            .collect();
        (picked, SelectionMode::Patterns)
    } else if let Some(preset) = preset {
        match preset {
            "vllm" => {
                let allow: Vec<String> = crate::patterns::VLLM_ALLOW
                    .iter()
                    .map(|pattern| pattern.to_string())
                    .collect();
                // The preset's own ignore table filters here; a user
                // --exclude is additionally applied by the universal
                // post-filter below (§2.2: exclude applies on top of every
                // mode).
                let ignore: Vec<String> = crate::patterns::VLLM_IGNORE
                    .iter()
                    .map(|pattern| pattern.to_string())
                    .collect();
                let picked = crate::patterns::filter_paths(tree, &allow, &ignore)
                    .into_iter()
                    .map(String::from)
                    .collect();
                (picked, SelectionMode::Preset)
            }
            other => {
                return Err(SyncSelectionError::UnknownPreset {
                    name: other.to_string(),
                })
            }
        }
    } else {
        (available(), SelectionMode::WholeRepo)
    };

    // §2.2: --exclude applies on top of every mode (filter_paths already
    // applied it for the Patterns/Preset paths; re-applying is idempotent).
    if !exclude.is_empty() {
        selected.retain(|path| {
            !exclude
                .iter()
                .any(|pattern| crate::patterns::fnmatch(pattern, path))
        });
    }

    if selected.is_empty() {
        return Err(SyncSelectionError::EmptySelection {
            available: available(),
        });
    }
    Ok((selected, mode))
}

/// FileDto listing of a repo tree (selection-error `available` payloads,
/// mirroring `resolve_files`' ambiguity lists through the shared
/// `FileSpec::from(&RepoFile)` mapping, W4.9).
//
// `pub(in crate::cli)` rather than `pub(super)`: read by the sibling
// `sync` submodule *and* pinned by `cli/tests.rs`'s mapping-fixture test.
pub(in crate::cli) fn tree_file_dtos(metadata: &ModelMetadata) -> Vec<FileDto> {
    metadata
        .siblings
        .iter()
        .filter(|f| !f.rfilename.ends_with('/'))
        .map(|f| FileDto::from(&FileSpec::from(f)))
        .collect()
}
