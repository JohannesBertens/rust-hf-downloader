//! TUI state enums (popup, filter preset, input, sort, pane focus, model
//! display mode) and the file-tree node rendered by `ui::render`.

use serde::{Deserialize, Serialize};

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
    AuthError { model_url: String },
    SearchPopup,
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
}

impl FocusedPane {
    /// Panes from which 'd' may trigger a download (the events.rs key
    /// guard's former OR-chain; Models covers the non-GGUF repository
    /// flow, FileTree the Standard-mode tree flow).
    pub fn accepts_download(&self) -> bool {
        matches!(
            self,
            Self::Models | Self::QuantizationGroups | Self::QuantizationFiles | Self::FileTree
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
        ] {
            assert_eq!(pane.accepts_download(), download, "{:?} download", pane);
            assert_eq!(pane.accepts_verify(), verify, "{:?} verify", pane);
        }
    }
}
