//! TUI state enums (popup, filter preset, input, sort, pane focus, model
//! display mode) and the file-tree node rendered by `ui::render`.

use serde::{Deserialize, Serialize};

use super::api::{FlarePackage, FlareRevision, RefKind};

/// Tree node for hierarchical file display
#[derive(Debug, Clone)]
pub struct FileTreeNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub children: Vec<FileTreeNode>,
    pub expanded: bool,
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupMode {
    None,
    DownloadPath,
    ResumeDownload,
    Options,
    AuthError {
        model_url: String,
    },
    SearchPopup,
    /// FLARE revision picker (plans/flare-models.md §3.5): the
    /// revision-list snapshot plus cursor, bound to the model id whose
    /// `flare_info` produced it. Enter re-resolves the FULL revision
    /// from `flare_info` by `target_commit` before enqueueing — the
    /// package files always come from the single source of truth.
    FlareRevision {
        model_id: String,
        choices: Vec<FlareRevisionChoice>,
        cursor: usize,
    },
}

/// One FLARE revision-picker popup row (plans/flare-models.md §3.5): a
/// display snapshot of one [`FlareRevision`]. A snapshot (not a live
/// reference) so the popup's list stays stable while `flare_info` may be
/// re-scanned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlareRevisionChoice {
    /// Preferred revision name (the directory segment + display name;
    /// never the pin — that is `target_commit`).
    pub revision: String,
    /// Other names sharing the commit (e.g. `["v3"]` when a tag points
    /// at the branch's commit).
    pub aliases: Vec<String>,
    /// Branch or Tag — rendered as the kind badge.
    pub kind: RefKind,
    /// 40-hex commit pin — Enter's re-resolution key.
    pub target_commit: String,
    /// Primary package filename (display only).
    pub package_name: String,
    /// Total package bytes (the packed file, or primary + companions in
    /// directory form).
    pub size: u64,
    /// Package file count.
    pub files: usize,
    /// The card's parsed arch token (e.g. `"gb202"`) — the render pass
    /// stars the row when it matches the session-detected platform.
    pub arch: Option<String>,
}

impl FlareRevisionChoice {
    /// Snapshot one scan revision for the picker popup
    /// (plans/flare-models.md §3.5).
    pub fn from_revision(revision: &FlareRevision) -> Self {
        let (package_name, size, files) = match &revision.package {
            FlarePackage::Packed { file } => {
                (file.rfilename.clone(), file.size.unwrap_or(0), 1usize)
            }
            FlarePackage::Directory {
                primary,
                companions,
            } => (
                primary.rfilename.clone(),
                primary.size.unwrap_or(0)
                    + companions.iter().map(|f| f.size.unwrap_or(0)).sum::<u64>(),
                1 + companions.len(),
            ),
        };
        Self {
            revision: revision.revision.clone(),
            aliases: revision.aliases.clone(),
            kind: revision.kind,
            target_commit: revision.target_commit.clone(),
            package_name,
            size,
            files,
            arch: revision.parsed.arch.clone(),
        }
    }
}

/// Filter presets for quick filter combinations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterPreset {
    NoFilters,
    Popular,
    HighlyRated,
    Recent,
}

/// Sort field options for model search
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SortField {
    #[default]
    Downloads,
    Likes,
    Modified,
    Name,
}

/// Sort direction (ascending or descending)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SortDirection {
    Ascending,
    #[default]
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Models,
    QuantizationGroups,
    QuantizationFiles,
    ModelMetadata,
    FileTree,
    /// FLARE mode's revision-cards panel (plans/flare-models.md §3.4): 'd' opens the
    /// revision-picker popup (download), 'v' does not apply — FLARE
    /// payloads verify through the LFS-oid digest like every download,
    /// there is no separate per-file verify action here.
    FlareRevisions,
}

impl FocusedPane {
    /// Panes from which 'd' may trigger a download (the events.rs key
    /// guard's former OR-chain; Models covers the non-GGUF repository
    /// flow, FileTree the Standard-mode tree flow).
    pub fn accepts_download(&self) -> bool {
        matches!(
            self,
            Self::Models
                | Self::QuantizationGroups
                | Self::QuantizationFiles
                | Self::FileTree
                | Self::FlareRevisions
        )
    }

    /// Panes from which 'v' may verify the selected downloaded file.
    pub fn accepts_verify(&self) -> bool {
        matches!(self, Self::QuantizationGroups | Self::QuantizationFiles)
    }
}

/// Model display mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelDisplayMode {
    Gguf,     // Show quantizations (current behavior)
    Standard, // Show metadata + file tree
    Flare,    // Show FLARE revision cards + repo facts (plans/flare-models.md §3.4)
}

#[cfg(test)]
mod tests {
    use super::FocusedPane;

    /// Pin the two pane sets exactly (W4.8): these replace the 'd'/'v'
    /// OR-chain guards, so membership must not drift.
    #[test]
    fn accepts_download_and_verify_pane_sets() {
        for (pane, download, verify) in [
            (FocusedPane::Models, true, false),
            (FocusedPane::QuantizationGroups, true, true),
            (FocusedPane::QuantizationFiles, true, true),
            (FocusedPane::ModelMetadata, false, false),
            (FocusedPane::FileTree, true, false),
            (FocusedPane::FlareRevisions, true, false),
        ] {
            assert_eq!(pane.accepts_download(), download, "{:?} download", pane);
            assert_eq!(pane.accepts_verify(), verify, "{:?} verify", pane);
        }
    }
}
