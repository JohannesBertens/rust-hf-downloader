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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
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

/// Model display mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelDisplayMode {
    Gguf,     // Show quantizations (current behavior)
    Standard, // Show metadata + file tree
}
