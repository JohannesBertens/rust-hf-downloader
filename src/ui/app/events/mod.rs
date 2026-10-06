//! Keyboard event dispatch and the shared navigation surface (W3.9):
//! [`App::on_key_event`] routes key presses by `PopupMode` to the private
//! [`keys`] submodule's handlers; this facade keeps the shared,
//! non-keyboard-specific App methods — list navigation (`next`/`previous`,
//! the quant/file/file-tree cursors via one `advance`), pane focus,
//! filter-value mutation (presets, save, `modify_focused_filter`),
//! `modify_option`, and file-tree expansion — plus the W4.6 navigation
//! contract tests. Mouse handling lives in `ui/app/mod.rs` (next to the
//! crossterm loop), not here.

mod keys;

use super::state::App;
use crate::models::*;
use crate::ui::render::{OptionsFieldId, OPTIONS_FIELDS};
use crate::ui::tree::toggle_node_expansion;
use crossterm::event::KeyEvent;
use ratatui::widgets::ListState;

impl App {
    /// Main keyboard event dispatcher
    pub async fn on_key_event(&mut self, key: KeyEvent) {
        *self.error.write() = None;

        // Handle popup input separately
        if self.popup_mode == PopupMode::SearchPopup {
            self.handle_search_popup_input(key).await;
            return;
        } else if self.popup_mode == PopupMode::Options {
            self.handle_options_popup_input(key).await;
            return;
        } else if self.popup_mode == PopupMode::ResumeDownload {
            self.handle_resume_popup_input(key).await;
            return;
        } else if self.popup_mode == PopupMode::DownloadPath {
            self.handle_download_path_popup_input(key).await;
            return;
        } else if matches!(self.popup_mode, PopupMode::AuthError { .. }) {
            self.handle_auth_error_popup_input(key).await;
            return;
        }

        self.handle_normal_mode_input(key).await;
    }

    /// Navigate to next model in list
    pub fn next(&mut self) {
        let models_len = self.models.read().len();
        advance(&mut self.list_state, models_len, true);
    }

    /// Navigate to previous model in list
    pub fn previous(&mut self) {
        let models_len = self.models.read().len();
        advance(&mut self.list_state, models_len, false);
    }

    /// Focus a specific pane and select first item if needed
    /// This is the core logic used by both toggle_focus() and mouse clicks
    pub fn focus_pane(&mut self, pane: FocusedPane) {
        // Skip if already focused on this pane
        if self.focused_pane == pane {
            return;
        }

        // Select first item in the target pane if it has items and none selected
        match pane {
            FocusedPane::Models => {
                // Models list - select first if available and none selected
                let models_len = self.models.read().len();
                if models_len > 0 && self.list_state.selected().is_none() {
                    self.list_state.select(Some(0));
                }
            }
            FocusedPane::QuantizationGroups => {
                // Quantization groups - select first if available and none selected
                let quants_len = self.quantizations.read().len();
                if quants_len > 0 && self.quant_list_state.selected().is_none() {
                    self.quant_list_state.select(Some(0));
                }
            }
            FocusedPane::QuantizationFiles => {
                // Quantization files - select first file if available and none selected
                if self.quant_file_list_state.selected().is_none() {
                    if let Some(selected_group) = self.quant_list_state.selected() {
                        let quantizations = self.quantizations.read();
                        if selected_group < quantizations.len()
                            && !quantizations[selected_group].files.is_empty()
                        {
                            self.quant_file_list_state.select(Some(0));
                        }
                    }
                }
            }
            FocusedPane::ModelMetadata => {
                // Metadata pane has no selection state
            }
            FocusedPane::FileTree => {
                // File tree - select first if available and none selected
                if self.file_tree_state.selected().is_none() {
                    let tree_has_items = self
                        .file_tree
                        .read()
                        .as_ref()
                        .map(|t| !t.children.is_empty())
                        .unwrap_or(false);
                    if tree_has_items {
                        self.file_tree_state.select(Some(0));
                    }
                }
            }
        }

        self.focused_pane = pane;
    }

    /// Toggle focus between panes based on display mode
    pub fn toggle_focus(&mut self) {
        let next_pane = match *self.display_mode.read() {
            ModelDisplayMode::Gguf => {
                // GGUF mode: cycle Models → QuantizationGroups → QuantizationFiles → Models
                match self.focused_pane {
                    FocusedPane::Models => FocusedPane::QuantizationGroups,
                    FocusedPane::QuantizationGroups => FocusedPane::QuantizationFiles,
                    FocusedPane::QuantizationFiles => FocusedPane::Models,
                    // Fallback for ModelMetadata/FileTree (shouldn't happen in GGUF mode)
                    _ => FocusedPane::Models,
                }
            }
            ModelDisplayMode::Standard => {
                // Standard mode: cycle Models → FileTree → Models (skip ModelMetadata - no actions)
                match self.focused_pane {
                    FocusedPane::Models => FocusedPane::FileTree,
                    FocusedPane::FileTree => FocusedPane::Models,
                    // Fallback for QuantizationGroups/Files/ModelMetadata (shouldn't happen in Standard mode)
                    _ => FocusedPane::Models,
                }
            }
        };

        self.focus_pane(next_pane);
    }

    /// Toggle focus between QuantizationGroups and QuantizationFiles panes
    pub fn toggle_quant_subfocus(&mut self) {
        match self.focused_pane {
            FocusedPane::QuantizationGroups => {
                // When switching to quantization files, select first file if available
                if let Some(selected_group) = self.quant_list_state.selected() {
                    let quantizations = self.quantizations.read().clone();
                    if selected_group < quantizations.len()
                        && !quantizations[selected_group].files.is_empty()
                    {
                        self.quant_file_list_state.select(Some(0));
                    }
                    self.focused_pane = FocusedPane::QuantizationFiles;
                }
            }
            FocusedPane::QuantizationFiles => {
                self.focused_pane = FocusedPane::QuantizationGroups;
            }
            _ => {}
        }
    }

    /// Navigate to next quantization in list
    pub fn next_quant(&mut self) {
        let len = self.quantizations.read().len();
        advance(&mut self.quant_list_state, len, true);
    }

    /// Navigate to previous quantization in list
    pub fn previous_quant(&mut self) {
        let len = self.quantizations.read().len();
        advance(&mut self.quant_list_state, len, false);
    }

    /// Navigate to next file in quantization files list
    pub fn next_file(&mut self) {
        if let Some(selected_group) = self.quant_list_state.selected() {
            let quantizations = self.quantizations.read().clone();

            if selected_group < quantizations.len() {
                advance(
                    &mut self.quant_file_list_state,
                    quantizations[selected_group].files.len(),
                    true,
                );
            }
        }
    }

    /// Navigate to previous file in quantization files list
    pub fn previous_file(&mut self) {
        if let Some(selected_group) = self.quant_list_state.selected() {
            let quantizations = self.quantizations.read().clone();

            if selected_group < quantizations.len() {
                advance(
                    &mut self.quant_file_list_state,
                    quantizations[selected_group].files.len(),
                    false,
                );
            }
        }
    }

    /// Modify focused filter field value
    pub fn modify_focused_filter(&mut self, delta: i32) {
        // Keyboard semantics: clamped table steps; on the sort field,
        // '+' cycles forward while '−' toggles the direction (see
        // FilterState::step for the divergence table)
        self.filters.step(self.focused_filter_field, delta);

        // Re-fetch with new filters (no status write on this path —
        // historical behavior, kept)
        self.apply_filter_refresh();
    }
    /// Apply a filter preset
    pub fn apply_filter_preset(&mut self, preset: crate::models::FilterPreset) {
        self.filters.apply_preset(preset);

        let status = match preset {
            crate::models::FilterPreset::NoFilters => "Preset: No Filters".to_string(),
            crate::models::FilterPreset::Popular => {
                "Preset: Popular (10k+ downloads, 100+ likes)".to_string()
            }
            crate::models::FilterPreset::HighlyRated => {
                "Preset: Highly Rated (1k+ likes)".to_string()
            }
            crate::models::FilterPreset::Recent => "Preset: Recent".to_string(),
        };
        *self.status.write() = status;

        // Apply preset by re-searching
        self.apply_filter_refresh();
    }

    /// Save current filter settings to config
    pub fn save_filter_settings(&mut self) {
        self.options.default_sort_field = self.filters.sort_field;
        self.options.default_sort_direction = self.filters.sort_direction;
        self.options.default_min_downloads = self.filters.min_downloads;
        self.options.default_min_likes = self.filters.min_likes;

        if let Err(e) = crate::config::save_config(&self.options) {
            *self.status.write() = format!("Failed to save filter settings: {}", e);
        } else {
            *self.status.write() = "Filter settings saved".to_string();
        }
    }

    /// Modify option value based on selected field and delta
    ///
    /// Field identity comes from the [`OPTIONS_FIELDS`] table (W4.7); the
    /// per-field step/clamp/toggle bodies below are the historical ones,
    /// kept arm-by-arm because they differ per field. A `selected_field`
    /// past the table (not reachable via the cursor bound) keeps the
    /// historical catch-all no-op.
    pub fn modify_option(&mut self, delta: i32) {
        let field = OPTIONS_FIELDS
            .get(self.options_dialog.selected_field)
            .map(|f| f.id);
        match field {
            None | Some(OptionsFieldId::DefaultDirectory) => {} // use Enter to edit
            Some(OptionsFieldId::HfToken) => {}                 // use Enter to edit
            Some(OptionsFieldId::ConcurrentThreads) => {
                // concurrent_threads (1-32)
                let new = (self.options.concurrent_threads as i32 + delta).clamp(1, 32) as usize;
                self.options.concurrent_threads = new;
            }
            Some(OptionsFieldId::NumChunks) => {
                // num_chunks (10-100)
                let new = (self.options.num_chunks as i32 + delta).clamp(10, 100) as usize;
                self.options.num_chunks = new;
            }
            Some(OptionsFieldId::MinChunkSize) => {
                // min_chunk_size (1MB-50MB)
                let step = 1024 * 1024; // 1MB
                let new = (self.options.min_chunk_size as i64 + delta as i64 * step)
                    .clamp(1024 * 1024, 50 * 1024 * 1024) as u64;
                self.options.min_chunk_size = new;
            }
            Some(OptionsFieldId::MaxChunkSize) => {
                // max_chunk_size (10MB-500MB)
                let step = 10 * 1024 * 1024; // 10MB
                let new = (self.options.max_chunk_size as i64 + delta as i64 * step)
                    .clamp(10 * 1024 * 1024, 500 * 1024 * 1024) as u64;
                self.options.max_chunk_size = new;
            }
            Some(OptionsFieldId::MaxRetries) => {
                // max_retries (0-10, step 1)
                let new = (self.options.max_retries as i32 + delta).clamp(0, 10) as u32;
                self.options.max_retries = new;
            }
            Some(OptionsFieldId::DownloadTimeoutSecs) => {
                // download_timeout_secs (60-600, step 30)
                let new = (self.options.download_timeout_secs as i64 + delta as i64 * 30)
                    .clamp(60, 600) as u64;
                self.options.download_timeout_secs = new;
            }
            Some(OptionsFieldId::RetryDelaySecs) => {
                // retry_delay_secs (1-10, step 1)
                let new = (self.options.retry_delay_secs as i64 + delta as i64).clamp(1, 10) as u64;
                self.options.retry_delay_secs = new;
            }
            Some(OptionsFieldId::ProgressUpdateIntervalMs) => {
                // progress_update_interval_ms (100-1000, step 50)
                let new = (self.options.progress_update_interval_ms as i64 + delta as i64 * 50)
                    .clamp(100, 1000) as u64;
                self.options.progress_update_interval_ms = new;
            }
            Some(OptionsFieldId::RateLimitEnabled) => {
                // download_rate_limit_enabled - toggle with +/-
                self.options.download_rate_limit_enabled =
                    !self.options.download_rate_limit_enabled;
            }
            Some(OptionsFieldId::RateLimitMbps) => {
                // download_rate_limit_mbps (0.1-1000.0, step 0.5)
                let new =
                    (self.options.download_rate_limit_mbps + delta as f64 * 0.5).clamp(0.1, 1000.0);
                self.options.download_rate_limit_mbps = new;
            }
            Some(OptionsFieldId::VerificationEnabled) => {
                // verification_on_completion - toggle with +/-
                self.options.verification_on_completion = !self.options.verification_on_completion;
            }
            Some(OptionsFieldId::ConcurrentVerifications) => {
                // concurrent_verifications (1-8, step 1)
                let new =
                    (self.options.concurrent_verifications as i32 + delta).clamp(1, 8) as usize;
                self.options.concurrent_verifications = new;
            }
            Some(OptionsFieldId::VerificationBufferSize) => {
                // verification_buffer_size (64KB-512KB, step 64KB)
                let step = 64 * 1024;
                let new = (self.options.verification_buffer_size as i64 + delta as i64 * step)
                    .clamp(64 * 1024, 512 * 1024) as usize;
                self.options.verification_buffer_size = new;
            }
            Some(OptionsFieldId::VerificationUpdateInterval) => {
                // verification_update_interval (50-500, step 50)
                let new = (self.options.verification_update_interval as i32 + delta * 50)
                    .clamp(50, 500) as usize;
                self.options.verification_update_interval = new;
            }
        }

        // Sync changes to global config immediately
        self.sync_options_to_config();

        // Save to disk
        if let Err(e) = crate::config::save_config(&self.options) {
            *self.status.write() = format!("Failed to save config: {}", e);
        }
    }

    /// Navigate to next item in file tree
    pub fn next_file_tree_item(&mut self) {
        let tree = self.file_tree.read();
        if let Some(tree) = tree.as_ref() {
            let len = crate::ui::tree::count_visible_nodes(tree);
            advance(&mut self.file_tree_state, len, true);
        }
    }

    /// Navigate to previous item in file tree
    pub fn previous_file_tree_item(&mut self) {
        let tree = self.file_tree.read();
        if let Some(tree) = tree.as_ref() {
            let len = crate::ui::tree::count_visible_nodes(tree);
            advance(&mut self.file_tree_state, len, false);
        }
    }

    /// Toggle expansion of directory in file tree
    pub fn toggle_file_tree_expansion(&mut self) {
        let selected_idx = match self.file_tree_state.selected() {
            Some(idx) => idx,
            None => return,
        };

        let mut tree = self.file_tree.read().clone();

        if let Some(ref mut tree) = tree {
            let flat = crate::ui::tree::flatten_tree_refs(tree);

            if selected_idx < flat.len() {
                let selected_path = flat[selected_idx].path.clone();

                // Find and toggle the node
                toggle_node_expansion(tree, &selected_path);

                // Update the tree
                *self.file_tree.write() = Some(tree.clone());
            }
        }
    }
}

/// Wrap-around cursor move shared by all list navigations (W4.6).
/// Contract pinned by the table tests in `mod tests` below: `len == 0` is a no-op;
/// an unselected list selects index 0 in BOTH directions; forward wraps last→first
/// (and any out-of-bounds selection → 0); backward wraps first→last (an
/// out-of-bounds selection stays `i - 1`, out of bounds — historical
/// behavior).
fn advance(state: &mut ListState, len: usize, forward: bool) {
    if len == 0 {
        return;
    }

    let i = match state.selected() {
        Some(i) => {
            if forward {
                if i >= len - 1 {
                    0
                } else {
                    i + 1
                }
            } else if i == 0 {
                len - 1
            } else {
                i - 1
            }
        }
        None => 0,
    };
    state.select(Some(i));
}

#[cfg(test)]
mod tests {
    //! W4.6 navigation table tests: len 0/1/3 × selection None/first/last ×
    //! direction, written FIRST against the old wrap-around fn bodies and
    //! required to pass unchanged after the `advance()` replacement. They
    //! pin: len 0 is a no-op; a single item wraps onto itself; an unselected
    //! list selects index 0 in BOTH directions (backward included — not
    //! `len - 1`); first/last wrap at both ends; and an out-of-bounds
    //! selection forward-wraps to 0 while backward keeps it out of bounds
    //! (`Some(i) - 1`), matching the historical match arms exactly.
    use super::*;
    use crate::models::{FileTreeNode, QuantizationGroup, QuantizationInfo};

    fn app_with_quants(n: usize) -> App {
        let app = App::new();
        *app.quantizations.write() = (0..n)
            .map(|i| QuantizationGroup {
                quant_type: format!("Q{}", i),
                files: Vec::new(),
                total_size: 0,
            })
            .collect();
        app
    }

    fn app_with_files(n: usize) -> App {
        let mut app = App::new();
        *app.quantizations.write() = vec![QuantizationGroup {
            quant_type: "Q".to_string(),
            files: (0..n)
                .map(|i| QuantizationInfo {
                    quant_type: "Q".to_string(),
                    filename: format!("f{}.gguf", i),
                    size: 0,
                    sha256: None,
                })
                .collect(),
            total_size: 0,
        }];
        app.quant_list_state.select(Some(0));
        app
    }

    fn app_with_tree(n: usize) -> App {
        let app = App::new();
        *app.file_tree.write() = Some(FileTreeNode {
            name: "root".to_string(),
            path: "root".to_string(),
            is_dir: true,
            size: None,
            children: (0..n)
                .map(|i| FileTreeNode {
                    name: format!("f{}", i),
                    path: format!("root/f{}", i),
                    is_dir: false,
                    size: None,
                    children: Vec::new(),
                    expanded: false,
                    depth: 1,
                })
                .collect(),
            expanded: true,
            depth: 0,
        });
        app
    }

    /// Drive the quantization-group cursor through the OLD public fns.
    fn quant_nav(len: usize, initial: Option<usize>, forward: bool) -> Option<usize> {
        let mut app = app_with_quants(len);
        app.quant_list_state.select(initial);
        if forward {
            app.next_quant();
        } else {
            app.previous_quant();
        }
        app.quant_list_state.selected()
    }

    /// Drive the quantization-file cursor (one group with `len` files).
    fn file_nav(len: usize, initial: Option<usize>, forward: bool) -> Option<usize> {
        let mut app = app_with_files(len);
        app.quant_file_list_state.select(initial);
        if forward {
            app.next_file();
        } else {
            app.previous_file();
        }
        app.quant_file_list_state.selected()
    }

    /// Drive the file-tree cursor (root with `len` flattened children).
    fn tree_nav(len: usize, initial: Option<usize>, forward: bool) -> Option<usize> {
        let mut app = app_with_tree(len);
        app.file_tree_state.select(initial);
        if forward {
            app.next_file_tree_item();
        } else {
            app.previous_file_tree_item();
        }
        app.file_tree_state.selected()
    }

    #[test]
    fn next_quant_table() {
        assert_eq!(quant_nav(0, None, true), None, "len 0: no-op");
        assert_eq!(
            quant_nav(0, Some(0), true),
            Some(0),
            "len 0: selection kept"
        );
        assert_eq!(quant_nav(1, None, true), Some(0), "None selects 0");
        assert_eq!(
            quant_nav(1, Some(0), true),
            Some(0),
            "single item wraps onto itself"
        );
        assert_eq!(quant_nav(3, None, true), Some(0));
        assert_eq!(quant_nav(3, Some(0), true), Some(1), "first → 1");
        assert_eq!(quant_nav(3, Some(1), true), Some(2), "middle → last");
        assert_eq!(quant_nav(3, Some(2), true), Some(0), "last wraps to first");
        assert_eq!(
            quant_nav(3, Some(5), true),
            Some(0),
            "OOB forward wraps to 0"
        );
    }

    #[test]
    fn previous_quant_table() {
        assert_eq!(quant_nav(0, None, false), None);
        assert_eq!(quant_nav(0, Some(0), false), Some(0));
        assert_eq!(
            quant_nav(1, None, false),
            Some(0),
            "None selects 0 even backward"
        );
        assert_eq!(quant_nav(1, Some(0), false), Some(0));
        assert_eq!(quant_nav(3, None, false), Some(0));
        assert_eq!(quant_nav(3, Some(0), false), Some(2), "first wraps to last");
        assert_eq!(quant_nav(3, Some(1), false), Some(0));
        assert_eq!(quant_nav(3, Some(2), false), Some(1));
        assert_eq!(
            quant_nav(3, Some(5), false),
            Some(4),
            "OOB backward stays OOB (i-1)"
        );
    }

    #[test]
    fn next_file_table() {
        assert_eq!(file_nav(0, None, true), None);
        assert_eq!(file_nav(0, Some(0), true), Some(0));
        assert_eq!(file_nav(1, None, true), Some(0));
        assert_eq!(file_nav(1, Some(0), true), Some(0));
        assert_eq!(file_nav(3, None, true), Some(0));
        assert_eq!(file_nav(3, Some(0), true), Some(1));
        assert_eq!(file_nav(3, Some(1), true), Some(2));
        assert_eq!(file_nav(3, Some(2), true), Some(0));
        assert_eq!(file_nav(3, Some(5), true), Some(0));
    }

    #[test]
    fn previous_file_table() {
        assert_eq!(file_nav(0, None, false), None);
        assert_eq!(file_nav(0, Some(0), false), Some(0));
        assert_eq!(file_nav(1, None, false), Some(0));
        assert_eq!(file_nav(1, Some(0), false), Some(0));
        assert_eq!(file_nav(3, None, false), Some(0));
        assert_eq!(file_nav(3, Some(0), false), Some(2));
        assert_eq!(file_nav(3, Some(1), false), Some(0));
        assert_eq!(file_nav(3, Some(2), false), Some(1));
        assert_eq!(file_nav(3, Some(5), false), Some(4));
    }

    #[test]
    fn next_file_tree_table() {
        assert_eq!(tree_nav(0, None, true), None);
        assert_eq!(tree_nav(0, Some(0), true), Some(0));
        assert_eq!(tree_nav(1, None, true), Some(0));
        assert_eq!(tree_nav(1, Some(0), true), Some(0));
        assert_eq!(tree_nav(3, None, true), Some(0));
        assert_eq!(tree_nav(3, Some(0), true), Some(1));
        assert_eq!(tree_nav(3, Some(1), true), Some(2));
        assert_eq!(tree_nav(3, Some(2), true), Some(0));
        assert_eq!(tree_nav(3, Some(5), true), Some(0));
    }

    #[test]
    fn previous_file_tree_table() {
        assert_eq!(tree_nav(0, None, false), None);
        assert_eq!(tree_nav(0, Some(0), false), Some(0));
        assert_eq!(tree_nav(1, None, false), Some(0));
        assert_eq!(tree_nav(1, Some(0), false), Some(0));
        assert_eq!(tree_nav(3, None, false), Some(0));
        assert_eq!(tree_nav(3, Some(0), false), Some(2));
        assert_eq!(tree_nav(3, Some(1), false), Some(0));
        assert_eq!(tree_nav(3, Some(2), false), Some(1));
        assert_eq!(tree_nav(3, Some(5), false), Some(4));
    }

    // (T3, test-hardening: the former `all_families_share_one_table` —
    // comparing the three navigation families' outputs to each other —
    // was deleted: post-W4.6 all three drive the ONE `advance()`, so the
    // comparison pinned nothing the per-family literal tables above
    // don't already pin.)
}
