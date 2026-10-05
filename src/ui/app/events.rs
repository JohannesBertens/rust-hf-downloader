use super::state::App;
use crate::models::*;
use crate::ui::tree::toggle_node_expansion;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use tui_input::backend::crossterm::EventHandler;

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

        match self.input_mode {
            InputMode::Normal => self.handle_normal_mode_input(key).await,
        }
    }

    /// Handle keyboard input in Normal mode
    async fn handle_normal_mode_input(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (_, KeyCode::Char('q'))
            | (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C')) => self.quit(),
            (_, KeyCode::Char('/')) => {
                // Open search popup instead of inline editing
                self.popup_mode = PopupMode::SearchPopup;
                self.input.reset(); // Clear previous search
                *self.status.write() = "Search Models".to_string();
            }
            (_, KeyCode::Char('d')) => {
                // Allow download from Models pane (for non-GGUF), QuantizationGroups,
                // QuantizationFiles, or the Standard-mode FileTree
                if self.focused_pane.accepts_download() {
                    self.trigger_download();
                }
            }
            (_, KeyCode::Char('v')) => {
                if self.focused_pane.accepts_verify() {
                    self.verify_downloaded_file().await;
                }
            }
            (_, KeyCode::Char('o')) => {
                self.popup_mode = PopupMode::Options;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('s') | KeyCode::Char('S')) => {
                // Save current filter settings as defaults
                self.save_filter_settings();
            }
            (_, KeyCode::Char('s')) => {
                // Cycle sort field: Downloads → Likes → Modified → Name → Downloads
                self.filters.cycle(0, true);

                // Re-fetch with new sort (status written AFTER the clear,
                // unlike the mouse sites — historical order, kept)
                self.apply_filter_refresh();

                *self.status.write() = format!("Sort by: {:?}", self.filters.sort_field);
            }
            (KeyModifiers::SHIFT, KeyCode::Char('S')) => {
                // Toggle sort direction
                self.filters.toggle_direction();

                // Re-fetch with new direction
                self.apply_filter_refresh();

                let arrow = match self.filters.sort_direction {
                    crate::models::SortDirection::Ascending => "▲",
                    crate::models::SortDirection::Descending => "▼",
                };
                *self.status.write() = format!(
                    "Sort direction: {:?} {}",
                    self.filters.sort_direction, arrow
                );
            }
            (_, KeyCode::Char('f')) => {
                // Cycle focused filter field
                self.focused_filter_field = (self.focused_filter_field + 1) % 3;
                let field_name = match self.focused_filter_field {
                    0 => "Sort",
                    1 => "Min Downloads",
                    2 => "Min Likes",
                    _ => unreachable!(),
                };
                *self.status.write() = format!("Focused filter: {}", field_name);
            }
            (_, KeyCode::Char('+')) if self.focused_pane == FocusedPane::Models => {
                // Increment focused filter (only in Models pane to avoid conflicts)
                self.modify_focused_filter(1);
            }
            (_, KeyCode::Char('-') | KeyCode::Char('_'))
                if self.focused_pane == FocusedPane::Models =>
            {
                // Decrement focused filter (only in Models pane to avoid conflicts)
                self.modify_focused_filter(-1);
            }
            (_, KeyCode::Char('r')) => {
                // Reset all filters to defaults
                self.filters.reset();
                self.focused_filter_field = 0;

                // Re-fetch with reset filters
                self.apply_filter_refresh();

                *self.status.write() = "Filters reset to defaults".to_string();
            }
            (_, KeyCode::Char('1')) => {
                // Preset 1: No Filters (default)
                if self.would_change_settings(FilterPreset::NoFilters) {
                    self.apply_filter_preset(FilterPreset::NoFilters);
                } else {
                    *self.status.write() = "Already using No Filters preset".to_string();
                }
            }
            (_, KeyCode::Char('2')) => {
                // Preset 2: Popular (10k+ downloads, 100+ likes)
                if self.would_change_settings(FilterPreset::Popular) {
                    self.apply_filter_preset(FilterPreset::Popular);
                } else {
                    *self.status.write() = "Already using Popular preset".to_string();
                }
            }
            (_, KeyCode::Char('3')) => {
                // Preset 3: Highly Rated (1k+ likes, sort by likes)
                if self.would_change_settings(FilterPreset::HighlyRated) {
                    self.apply_filter_preset(FilterPreset::HighlyRated);
                } else {
                    *self.status.write() = "Already using Highly Rated preset".to_string();
                }
            }
            (_, KeyCode::Char('4')) => {
                // Preset 4: Recent (sort by modified)
                if self.would_change_settings(FilterPreset::Recent) {
                    self.apply_filter_preset(FilterPreset::Recent);
                } else {
                    *self.status.write() = "Already using Recent preset".to_string();
                }
            }
            (_, KeyCode::Tab) => {
                self.toggle_focus();
            }
            (_, KeyCode::Left) => {
                // Left arrow: switch from QuantizationFiles to QuantizationGroups
                if self.focused_pane == FocusedPane::QuantizationFiles {
                    self.toggle_quant_subfocus();
                }
            }
            (_, KeyCode::Right) => {
                // Right arrow: switch from QuantizationGroups to QuantizationFiles
                if self.focused_pane == FocusedPane::QuantizationGroups {
                    self.toggle_quant_subfocus();
                }
            }
            (_, KeyCode::Down | KeyCode::Char('j')) => {
                match self.focused_pane {
                    FocusedPane::Models => {
                        self.next();
                        // Clear details immediately to show selection change
                        self.clear_model_details();
                        // Set flag to load on next iteration (allows UI to render first)
                        self.needs_load_quantizations = true;
                    }
                    FocusedPane::QuantizationGroups => {
                        self.next_quant();
                    }
                    FocusedPane::QuantizationFiles => {
                        self.next_file();
                    }
                    FocusedPane::ModelMetadata => {
                        // No navigation in metadata pane (read-only text)
                    }
                    FocusedPane::FileTree => {
                        self.next_file_tree_item();
                    }
                }
            }
            (_, KeyCode::Up | KeyCode::Char('k')) => {
                match self.focused_pane {
                    FocusedPane::Models => {
                        self.previous();
                        // Clear details immediately to show selection change
                        self.clear_model_details();
                        // Set flag to load on next iteration (allows UI to render first)
                        self.needs_load_quantizations = true;
                    }
                    FocusedPane::QuantizationGroups => {
                        self.previous_quant();
                    }
                    FocusedPane::QuantizationFiles => {
                        self.previous_file();
                    }
                    FocusedPane::ModelMetadata => {
                        // No navigation in metadata pane (read-only text)
                    }
                    FocusedPane::FileTree => {
                        self.previous_file_tree_item();
                    }
                }
            }
            (_, KeyCode::Enter) => {
                match self.focused_pane {
                    FocusedPane::Models => {
                        // Show model details first
                        self.show_model_details().await;
                        // Switch focus to the appropriate pane based on display mode
                        // (toggle_focus already handles skipping ModelMetadata in Standard mode)
                        self.toggle_focus();
                    }
                    FocusedPane::QuantizationGroups => {
                        self.show_quantization_details().await;
                    }
                    FocusedPane::QuantizationFiles => {
                        self.show_file_details().await;
                    }
                    FocusedPane::ModelMetadata => {
                        // No action on Enter in metadata pane
                    }
                    FocusedPane::FileTree => {
                        self.toggle_file_tree_expansion();
                    }
                }
            }
            _ => {}
        }
    }

    /// Handle keyboard input in Search popup
    async fn handle_search_popup_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.input_mode = InputMode::Normal;
                self.popup_mode = PopupMode::None;
                // Clear results immediately before searching
                self.clear_search_results();
                self.needs_search_models = true;
            }
            KeyCode::Esc => {
                self.popup_mode = PopupMode::None;
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Char(c) => {
                self.input.handle(tui_input::InputRequest::InsertChar(c));
            }
            KeyCode::Backspace => {
                self.input.handle(tui_input::InputRequest::DeletePrevChar);
            }
            KeyCode::Delete => {
                self.input.handle(tui_input::InputRequest::DeleteNextChar);
            }
            KeyCode::Left => {
                self.input.handle(tui_input::InputRequest::GoToPrevChar);
            }
            KeyCode::Right => {
                self.input.handle(tui_input::InputRequest::GoToNextChar);
            }
            KeyCode::Home => {
                self.input.handle(tui_input::InputRequest::GoToStart);
            }
            KeyCode::End => {
                self.input.handle(tui_input::InputRequest::GoToEnd);
            }
            _ => {}
        }
    }

    /// Handle keyboard input in Options popup
    async fn handle_options_popup_input(&mut self, key: KeyEvent) {
        // If editing token, handle text input
        if self.options.editing_token {
            match key.code {
                KeyCode::Enter => {
                    // Save the edited token (empty string becomes None)
                    let new_token = self.options_token_input.value().to_string();
                    self.options.hf_token = if new_token.is_empty() {
                        None
                    } else {
                        Some(new_token)
                    };
                    self.options.editing_token = false;

                    // Save to disk
                    if let Err(e) = crate::config::save_config(&self.options) {
                        *self.status.write() = format!("Failed to save config: {}", e);
                    }
                }
                KeyCode::Esc => {
                    // Cancel editing
                    self.options.editing_token = false;
                }
                _ => {
                    self.options_token_input.handle_event(&Event::Key(key));
                }
            }
        } else if self.options.editing_directory {
            match key.code {
                KeyCode::Enter => {
                    // Save the edited directory
                    self.options.default_directory =
                        self.options_directory_input.value().to_string();
                    self.options.editing_directory = false;

                    // Save to disk
                    if let Err(e) = crate::config::save_config(&self.options) {
                        *self.status.write() = format!("Failed to save config: {}", e);
                    }
                }
                KeyCode::Esc => {
                    // Cancel editing
                    self.options.editing_directory = false;
                }
                _ => {
                    self.options_directory_input.handle_event(&Event::Key(key));
                }
            }
        } else {
            // Normal navigation mode
            match key.code {
                KeyCode::Esc => {
                    self.popup_mode = PopupMode::None;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if self.options.selected_field > 0 {
                        self.options.selected_field -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.options.selected_field < 15 {
                        self.options.selected_field += 1;
                    }
                }
                KeyCode::Char('+') | KeyCode::Right => {
                    self.modify_option(1);
                }
                KeyCode::Char('-') | KeyCode::Left => {
                    self.modify_option(-1);
                }
                KeyCode::Enter => {
                    // Enter edit mode for directory or token field
                    if self.options.selected_field == 0 {
                        self.options.editing_directory = true;
                        self.options_directory_input = tui_input::Input::default()
                            .with_value(self.options.default_directory.clone());
                    } else if self.options.selected_field == 1 {
                        self.options.editing_token = true;
                        self.options_token_input = tui_input::Input::default()
                            .with_value(self.options.hf_token.as_deref().unwrap_or("").to_string());
                    }
                }
                _ => {}
            }
        }
    }

    /// Handle keyboard input in Resume Download popup
    async fn handle_resume_popup_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.resume_incomplete_downloads().await;
                self.popup_mode = PopupMode::None;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.popup_mode = PopupMode::None;
                self.incomplete_downloads.clear();
                *self.status.write() = "Skipped incomplete downloads".to_string();
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                self.delete_incomplete_downloads().await;
                self.popup_mode = PopupMode::None;
            }
            _ => {}
        }
    }

    /// Handle keyboard input in Download Path popup
    async fn handle_download_path_popup_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.confirm_download().await;
                self.popup_mode = PopupMode::None;
            }
            KeyCode::Esc => {
                self.pending_tree_download = None;
                self.popup_mode = PopupMode::None;
                *self.status.write() = "Download cancelled".to_string();
            }
            _ => {
                self.download_path_input.handle_event(&Event::Key(key));
            }
        }
    }

    /// Handle keyboard input in Authentication Error popup
    async fn handle_auth_error_popup_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.popup_mode = PopupMode::None;
            }
            KeyCode::Char('o') => {
                // Dismiss auth popup and open options
                self.popup_mode = PopupMode::Options;
            }
            _ => {}
        }
    }

    /// Navigate to next model in list
    pub fn next(&mut self) {
        let models_len = self.models.read().len();

        if models_len == 0 {
            return;
        }

        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= models_len - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    /// Navigate to previous model in list
    pub fn previous(&mut self) {
        let models_len = self.models.read().len();

        if models_len == 0 {
            return;
        }

        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    models_len - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
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

    /// Check if applying a preset would change the current settings
    /// Returns true if the preset settings differ from current settings
    fn would_change_settings(&self, preset: crate::models::FilterPreset) -> bool {
        !self.filters.matches_preset(preset)
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
    pub fn modify_option(&mut self, delta: i32) {
        match self.options.selected_field {
            0 => {} // default_directory - use Enter to edit
            1 => {} // hf_token - use Enter to edit
            2 => {
                // concurrent_threads (1-32)
                let new = (self.options.concurrent_threads as i32 + delta).clamp(1, 32) as usize;
                self.options.concurrent_threads = new;
            }
            3 => {
                // num_chunks (10-100)
                let new = (self.options.num_chunks as i32 + delta).clamp(10, 100) as usize;
                self.options.num_chunks = new;
            }
            4 => {
                // min_chunk_size (1MB-50MB)
                let step = 1024 * 1024; // 1MB
                let new = (self.options.min_chunk_size as i64 + delta as i64 * step)
                    .clamp(1024 * 1024, 50 * 1024 * 1024) as u64;
                self.options.min_chunk_size = new;
            }
            5 => {
                // max_chunk_size (10MB-500MB)
                let step = 10 * 1024 * 1024; // 10MB
                let new = (self.options.max_chunk_size as i64 + delta as i64 * step)
                    .clamp(10 * 1024 * 1024, 500 * 1024 * 1024) as u64;
                self.options.max_chunk_size = new;
            }
            6 => {
                // max_retries (0-10, step 1)
                let new = (self.options.max_retries as i32 + delta).clamp(0, 10) as u32;
                self.options.max_retries = new;
            }
            7 => {
                // download_timeout_secs (60-600, step 30)
                let new = (self.options.download_timeout_secs as i64 + delta as i64 * 30)
                    .clamp(60, 600) as u64;
                self.options.download_timeout_secs = new;
            }
            8 => {
                // retry_delay_secs (1-10, step 1)
                let new = (self.options.retry_delay_secs as i64 + delta as i64).clamp(1, 10) as u64;
                self.options.retry_delay_secs = new;
            }
            9 => {
                // progress_update_interval_ms (100-1000, step 50)
                let new = (self.options.progress_update_interval_ms as i64 + delta as i64 * 50)
                    .clamp(100, 1000) as u64;
                self.options.progress_update_interval_ms = new;
            }
            10 => {
                // download_rate_limit_enabled - toggle with +/-
                self.options.download_rate_limit_enabled =
                    !self.options.download_rate_limit_enabled;
            }
            11 => {
                // download_rate_limit_mbps (0.1-1000.0, step 0.5)
                let new =
                    (self.options.download_rate_limit_mbps + delta as f64 * 0.5).clamp(0.1, 1000.0);
                self.options.download_rate_limit_mbps = new;
            }
            12 => {
                // verification_on_completion - toggle with +/-
                self.options.verification_on_completion = !self.options.verification_on_completion;
            }
            13 => {
                // concurrent_verifications (1-8, step 1)
                let new =
                    (self.options.concurrent_verifications as i32 + delta).clamp(1, 8) as usize;
                self.options.concurrent_verifications = new;
            }
            14 => {
                // verification_buffer_size (64KB-512KB, step 64KB)
                let step = 64 * 1024;
                let new = (self.options.verification_buffer_size as i64 + delta as i64 * step)
                    .clamp(64 * 1024, 512 * 1024) as usize;
                self.options.verification_buffer_size = new;
            }
            15 => {
                // verification_update_interval (50-500, step 50)
                let new = (self.options.verification_update_interval as i32 + delta * 50)
                    .clamp(50, 500) as usize;
                self.options.verification_update_interval = new;
            }
            _ => {}
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
        let tree = self.file_tree.read().clone();

        if let Some(tree) = tree {
            let len = crate::ui::tree::flatten_tree_for_navigation(&tree).len();
            advance(&mut self.file_tree_state, len, true);
        }
    }

    /// Navigate to previous item in file tree
    pub fn previous_file_tree_item(&mut self) {
        let tree = self.file_tree.read().clone();

        if let Some(tree) = tree {
            let len = crate::ui::tree::flatten_tree_for_navigation(&tree).len();
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
            let flat = crate::ui::tree::flatten_tree_for_navigation(tree);

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

/// Wrap-around cursor move shared by the quantization-group,
/// quantization-file and file-tree lists (W4.6 — six per-fn copies of
/// this match collapsed into one). Contract pinned by the table tests in
/// `mod tests` below: `len == 0` is a no-op; an unselected list selects
/// index 0 in BOTH directions; forward wraps last→first (and any
/// out-of-bounds selection → 0); backward wraps first→last (an
/// out-of-bounds selection stays `i - 1`, out of bounds — historical
/// behavior). The Models list keeps its own `next`/`previous` methods
/// (their call sites also clear model details and trigger a quant
/// reload).
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

    /// The three cursor families are table-identical — the property that
    /// lets ONE advance() serve all of them.
    #[test]
    fn all_families_share_one_table() {
        for len in [0usize, 1, 3] {
            for initial in [None, Some(0), Some(len.saturating_sub(1)), Some(len + 2)] {
                for forward in [true, false] {
                    // Skip first/last variants that collapse onto other rows.
                    if initial == Some(len + 2) && len == 0 {
                        continue;
                    }
                    assert_eq!(
                        quant_nav(len, initial, forward),
                        file_nav(len, initial, forward),
                        "quant vs file: len={} initial={:?} forward={}",
                        len,
                        initial,
                        forward
                    );
                    assert_eq!(
                        quant_nav(len, initial, forward),
                        tree_nav(len, initial, forward),
                        "quant vs tree: len={} initial={:?} forward={}",
                        len,
                        initial,
                        forward
                    );
                }
            }
        }
    }
}
