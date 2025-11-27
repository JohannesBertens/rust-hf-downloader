use super::state::App;
use crate::models::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tui_input::backend::crossterm::EventHandler;

impl App {
    /// Main keyboard event dispatcher
    pub async fn on_key_event(&mut self, key: KeyEvent) {
        *self.error.write().unwrap() = None;

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
            InputMode::Editing => self.handle_editing_mode_input(key).await,
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
                *self.status.write().unwrap() = "Search Models".to_string();
            }
            (_, KeyCode::Char('d')) => {
                // Allow download from Models pane (for non-GGUF), QuantizationGroups, or QuantizationFiles
                if self.focused_pane == FocusedPane::Models || 
                   self.focused_pane == FocusedPane::QuantizationGroups || 
                   self.focused_pane == FocusedPane::QuantizationFiles {
                    self.trigger_download();
                }
            }
            (_, KeyCode::Char('v')) => {
                if self.focused_pane == FocusedPane::QuantizationGroups || self.focused_pane == FocusedPane::QuantizationFiles {
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
                self.sort_field = match self.sort_field {
                    crate::models::SortField::Downloads => crate::models::SortField::Likes,
                    crate::models::SortField::Likes => crate::models::SortField::Modified,
                    crate::models::SortField::Modified => crate::models::SortField::Name,
                    crate::models::SortField::Name => crate::models::SortField::Downloads,
                };
                
                // Re-fetch with new sort
                self.clear_search_results();
                self.needs_search_models = true;
                
                *self.status.write().unwrap() = format!("Sort by: {:?}", self.sort_field);
            }
            (KeyModifiers::SHIFT, KeyCode::Char('S')) => {
                // Toggle sort direction
                self.sort_direction = match self.sort_direction {
                    crate::models::SortDirection::Ascending => crate::models::SortDirection::Descending,
                    crate::models::SortDirection::Descending => crate::models::SortDirection::Ascending,
                };
                
                // Re-fetch with new direction
                self.clear_search_results();
                self.needs_search_models = true;
                
                let arrow = match self.sort_direction {
                    crate::models::SortDirection::Ascending => "▲",
                    crate::models::SortDirection::Descending => "▼",
                };
                *self.status.write().unwrap() = format!("Sort direction: {:?} {}", self.sort_direction, arrow);
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
                *self.status.write().unwrap() = format!("Focused filter: {}", field_name);
            }
            (_, KeyCode::Char('+')) if self.focused_pane == FocusedPane::Models => {
                // Increment focused filter (only in Models pane to avoid conflicts)
                self.modify_focused_filter(1);
            }
            (_, KeyCode::Char('-') | KeyCode::Char('_')) if self.focused_pane == FocusedPane::Models => {
                // Decrement focused filter (only in Models pane to avoid conflicts)
                self.modify_focused_filter(-1);
            }
            (_, KeyCode::Char('r')) => {
                // Reset all filters to defaults
                self.sort_field = crate::models::SortField::default();
                self.sort_direction = crate::models::SortDirection::default();
                self.filter_min_downloads = 0;
                self.filter_min_likes = 0;
                self.focused_filter_field = 0;
                
                // Re-fetch with reset filters
                self.clear_search_results();
                self.needs_search_models = true;
                
                *self.status.write().unwrap() = "Filters reset to defaults".to_string();
            }
            (_, KeyCode::Char('1')) => {
                // Preset 1: No Filters (default)
                if self.would_change_settings(FilterPreset::NoFilters) {
                    self.apply_filter_preset(FilterPreset::NoFilters);
                } else {
                    *self.status.write().unwrap() = "Already using No Filters preset".to_string();
                }
            }
            (_, KeyCode::Char('2')) => {
                // Preset 2: Popular (10k+ downloads, 100+ likes)
                if self.would_change_settings(FilterPreset::Popular) {
                    self.apply_filter_preset(FilterPreset::Popular);
                } else {
                    *self.status.write().unwrap() = "Already using Popular preset".to_string();
                }
            }
            (_, KeyCode::Char('3')) => {
                // Preset 3: Highly Rated (1k+ likes, sort by likes)
                if self.would_change_settings(FilterPreset::HighlyRated) {
                    self.apply_filter_preset(FilterPreset::HighlyRated);
                } else {
                    *self.status.write().unwrap() = "Already using Highly Rated preset".to_string();
                }
            }
            (_, KeyCode::Char('4')) => {
                // Preset 4: Recent (sort by modified)
                if self.would_change_settings(FilterPreset::Recent) {
                    self.apply_filter_preset(FilterPreset::Recent);
                } else {
                    *self.status.write().unwrap() = "Already using Recent preset".to_string();
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

    /// Handle keyboard input in Editing mode
    async fn handle_editing_mode_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.input_mode = InputMode::Normal;
                // Clear results immediately before searching
                self.clear_search_results();
                // Set flag to search on next iteration (allows UI to render first)
                self.needs_search_models = true;
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                *self.status.write().unwrap() = "Press '/' to search, Tab to switch lists, 'd' to download, 'v' to verify, 'o' for options, 'q' to quit".to_string();
            }
            _ => {
                self.input.handle_event(&Event::Key(key));
            }
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
                        *self.status.write().unwrap() = format!("Failed to save config: {}", e);
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
                    self.options.default_directory = self.options_directory_input.value().to_string();
                    self.options.editing_directory = false;
                    
                    // Save to disk
                    if let Err(e) = crate::config::save_config(&self.options) {
                        *self.status.write().unwrap() = format!("Failed to save config: {}", e);
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
                    if self.options.selected_field < 13 {
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
                *self.status.write().unwrap() = "Skipped incomplete downloads".to_string();
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
                self.popup_mode = PopupMode::None;
                *self.status.write().unwrap() = "Download cancelled".to_string();
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
        let models_len = futures::executor::block_on(async {
            self.models.read().unwrap().len()
        });
        
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
        let models_len = futures::executor::block_on(async {
            self.models.read().unwrap().len()
        });
        
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

    /// Toggle focus between panes based on display mode
    pub fn toggle_focus(&mut self) {
        self.focused_pane = match *self.display_mode.read().unwrap() {
            ModelDisplayMode::Gguf => {
                // GGUF mode: cycle Models → QuantizationGroups → QuantizationFiles → Models
                match self.focused_pane {
                    FocusedPane::Models => {
                        // When switching to quantization groups, select first item if available
                        let quants_len = futures::executor::block_on(async {
                            self.quantizations.read().unwrap().len()
                        });
                        if quants_len > 0 {
                            self.quant_list_state.select(Some(0));
                        }
                        FocusedPane::QuantizationGroups
                    }
                    FocusedPane::QuantizationGroups => {
                        // When switching to quantization files, select first file if available
                        if let Some(selected_group) = self.quant_list_state.selected() {
                            let quantizations = futures::executor::block_on(async {
                                self.quantizations.read().unwrap().clone()
                            });
                            if selected_group < quantizations.len() && !quantizations[selected_group].files.is_empty() {
                                self.quant_file_list_state.select(Some(0));
                            }
                        }
                        FocusedPane::QuantizationFiles
                    }
                    FocusedPane::QuantizationFiles => FocusedPane::Models,
                    // Fallback for ModelMetadata/FileTree (shouldn't happen in GGUF mode)
                    _ => FocusedPane::Models,
                }
            }
            ModelDisplayMode::Standard => {
                // Standard mode: cycle Models → FileTree → Models (skip ModelMetadata - no actions)
                match self.focused_pane {
                    FocusedPane::Models => {
                        // Skip directly to FileTree, select first item if available
                        let tree_has_items = futures::executor::block_on(async {
                            self.file_tree.read().unwrap().as_ref()
                                .map(|t| !t.children.is_empty())
                                .unwrap_or(false)
                        });
                        if tree_has_items {
                            self.file_tree_state.select(Some(0));
                        }
                        FocusedPane::FileTree
                    }
                    FocusedPane::FileTree => FocusedPane::Models,
                    // Fallback for QuantizationGroups/Files/ModelMetadata (shouldn't happen in Standard mode)
                    _ => FocusedPane::Models,
                }
            }
        };
    }

    /// Toggle focus between QuantizationGroups and QuantizationFiles panes
    pub fn toggle_quant_subfocus(&mut self) {
        match self.focused_pane {
            FocusedPane::QuantizationGroups => {
                // When switching to quantization files, select first file if available
                if let Some(selected_group) = self.quant_list_state.selected() {
                    let quantizations = futures::executor::block_on(async {
                        self.quantizations.read().unwrap().clone()
                    });
                    if selected_group < quantizations.len() && !quantizations[selected_group].files.is_empty() {
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
        let quants_len = futures::executor::block_on(async {
            self.quantizations.read().unwrap().len()
        });
        
        if quants_len == 0 {
            return;
        }
        
        let i = match self.quant_list_state.selected() {
            Some(i) => {
                if i >= quants_len - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.quant_list_state.select(Some(i));
    }

    /// Navigate to previous quantization in list
    pub fn previous_quant(&mut self) {
        let quants_len = futures::executor::block_on(async {
            self.quantizations.read().unwrap().len()
        });
        
        if quants_len == 0 {
            return;
        }
        
        let i = match self.quant_list_state.selected() {
            Some(i) => {
                if i == 0 {
                    quants_len - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.quant_list_state.select(Some(i));
    }

    /// Navigate to next file in quantization files list
    pub fn next_file(&mut self) {
        if let Some(selected_group) = self.quant_list_state.selected() {
            let quantizations = futures::executor::block_on(async {
                self.quantizations.read().unwrap().clone()
            });
            
            if selected_group < quantizations.len() {
                let files_len = quantizations[selected_group].files.len();
                
                if files_len == 0 {
                    return;
                }
                
                let i = match self.quant_file_list_state.selected() {
                    Some(i) => {
                        if i >= files_len - 1 {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };
                self.quant_file_list_state.select(Some(i));
            }
        }
    }

    /// Navigate to previous file in quantization files list
    pub fn previous_file(&mut self) {
        if let Some(selected_group) = self.quant_list_state.selected() {
            let quantizations = futures::executor::block_on(async {
                self.quantizations.read().unwrap().clone()
            });
            
            if selected_group < quantizations.len() {
                let files_len = quantizations[selected_group].files.len();
                
                if files_len == 0 {
                    return;
                }
                
                let i = match self.quant_file_list_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            files_len - 1
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };
                self.quant_file_list_state.select(Some(i));
            }
        }
    }

    /// Modify focused filter field value
    pub fn modify_focused_filter(&mut self, delta: i32) {
        match self.focused_filter_field {
            0 => {
                // Sort field cycling
                if delta > 0 {
                    self.sort_field = match self.sort_field {
                        crate::models::SortField::Downloads => crate::models::SortField::Likes,
                        crate::models::SortField::Likes => crate::models::SortField::Modified,
                        crate::models::SortField::Modified => crate::models::SortField::Name,
                        crate::models::SortField::Name => crate::models::SortField::Downloads,
                    };
                } else {
                    // Toggle direction with -
                    self.sort_direction = match self.sort_direction {
                        crate::models::SortDirection::Ascending => crate::models::SortDirection::Descending,
                        crate::models::SortDirection::Descending => crate::models::SortDirection::Ascending,
                    };
                }
            }
            1 => {
                // Min downloads: 0, 100, 1k, 10k, 100k, 1M
                let steps = [0, 100, 1_000, 10_000, 100_000, 1_000_000];
                let current_idx = steps.iter().position(|&x| x == self.filter_min_downloads).unwrap_or(0);
                let new_idx = if delta > 0 {
                    (current_idx + 1).min(steps.len() - 1)
                } else {
                    current_idx.saturating_sub(1)
                };
                self.filter_min_downloads = steps[new_idx];
            }
            2 => {
                // Min likes: 0, 10, 50, 100, 500, 1k, 5k
                let steps = [0, 10, 50, 100, 500, 1_000, 5_000];
                let current_idx = steps.iter().position(|&x| x == self.filter_min_likes).unwrap_or(0);
                let new_idx = if delta > 0 {
                    (current_idx + 1).min(steps.len() - 1)
                } else {
                    current_idx.saturating_sub(1)
                };
                self.filter_min_likes = steps[new_idx];
            }
            _ => {}
        }
        
        // Re-fetch with new filters
        self.clear_search_results();
        self.needs_search_models = true;
    }

    /// Check if applying a preset would change the current settings
    /// Returns true if the preset settings differ from current settings
    fn would_change_settings(&self, preset: crate::models::FilterPreset) -> bool {
        use crate::models::FilterPreset;
        
        let (target_sort_field, target_sort_direction, target_min_downloads, target_min_likes) = match preset {
            FilterPreset::NoFilters => {
                (SortField::Downloads, SortDirection::Descending, 0, 0)
            }
            FilterPreset::Popular => {
                (SortField::Downloads, SortDirection::Descending, 10_000, 100)
            }
            FilterPreset::HighlyRated => {
                (SortField::Likes, SortDirection::Descending, 0, 1_000)
            }
            FilterPreset::Recent => {
                (SortField::Modified, SortDirection::Descending, 0, 0)
            }
        };
        
        self.sort_field != target_sort_field ||
        self.sort_direction != target_sort_direction ||
        self.filter_min_downloads != target_min_downloads ||
        self.filter_min_likes != target_min_likes
    }

    /// Apply a filter preset
    pub fn apply_filter_preset(&mut self, preset: crate::models::FilterPreset) {
        use crate::models::FilterPreset;
        
        match preset {
            FilterPreset::NoFilters => {
                // Default: downloads descending, no filters
                self.sort_field = SortField::Downloads;
                self.sort_direction = SortDirection::Descending;
                self.filter_min_downloads = 0;
                self.filter_min_likes = 0;
                *self.status.write().unwrap() = "Preset: No Filters".to_string();
            }
            FilterPreset::Popular => {
                // Popular models: 10k+ downloads, 100+ likes
                self.sort_field = SortField::Downloads;
                self.sort_direction = SortDirection::Descending;
                self.filter_min_downloads = 10_000;
                self.filter_min_likes = 100;
                *self.status.write().unwrap() = "Preset: Popular (10k+ downloads, 100+ likes)".to_string();
            }
            FilterPreset::HighlyRated => {
                // Highly rated: 1k+ likes, sorted by likes
                self.sort_field = SortField::Likes;
                self.sort_direction = SortDirection::Descending;
                self.filter_min_downloads = 0;
                self.filter_min_likes = 1_000;
                *self.status.write().unwrap() = "Preset: Highly Rated (1k+ likes)".to_string();
            }
            FilterPreset::Recent => {
                // Recently updated
                self.sort_field = SortField::Modified;
                self.sort_direction = SortDirection::Descending;
                self.filter_min_downloads = 0;
                self.filter_min_likes = 0;
                *self.status.write().unwrap() = "Preset: Recent".to_string();
            }
        }
        
        // Apply preset by re-searching
        self.clear_search_results();
        self.needs_search_models = true;
    }

    /// Save current filter settings to config
    pub fn save_filter_settings(&mut self) {
        self.options.default_sort_field = self.sort_field;
        self.options.default_sort_direction = self.sort_direction;
        self.options.default_min_downloads = self.filter_min_downloads;
        self.options.default_min_likes = self.filter_min_likes;
        
        if let Err(e) = crate::config::save_config(&self.options) {
            *self.status.write().unwrap() = format!("Failed to save filter settings: {}", e);
        } else {
            *self.status.write().unwrap() = "Filter settings saved".to_string();
        }
    }

    /// Modify option value based on selected field and delta
    pub fn modify_option(&mut self, delta: i32) {
        match self.options.selected_field {
            0 => {} // default_directory - use Enter to edit
            1 => {} // hf_token - use Enter to edit
            2 => { // concurrent_threads (1-32)
                let new = (self.options.concurrent_threads as i32 + delta)
                    .clamp(1, 32) as usize;
                self.options.concurrent_threads = new;
            }
            3 => { // num_chunks (10-100)
                let new = (self.options.num_chunks as i32 + delta)
                    .clamp(10, 100) as usize;
                self.options.num_chunks = new;
            }
            4 => { // min_chunk_size (1MB-50MB)
                let step = 1024 * 1024; // 1MB
                let new = (self.options.min_chunk_size as i64 + delta as i64 * step)
                    .clamp(1024 * 1024, 50 * 1024 * 1024) as u64;
                self.options.min_chunk_size = new;
            }
            5 => { // max_chunk_size (10MB-500MB)
                let step = 10 * 1024 * 1024; // 10MB
                let new = (self.options.max_chunk_size as i64 + delta as i64 * step)
                    .clamp(10 * 1024 * 1024, 500 * 1024 * 1024) as u64;
                self.options.max_chunk_size = new;
            }
            6 => { // max_retries (0-10, step 1)
                let new = (self.options.max_retries as i32 + delta)
                    .clamp(0, 10) as u32;
                self.options.max_retries = new;
            }
            7 => { // download_timeout_secs (60-600, step 30)
                let new = (self.options.download_timeout_secs as i64 + delta as i64 * 30)
                    .clamp(60, 600) as u64;
                self.options.download_timeout_secs = new;
            }
            8 => { // retry_delay_secs (1-10, step 1)
                let new = (self.options.retry_delay_secs as i64 + delta as i64)
                    .clamp(1, 10) as u64;
                self.options.retry_delay_secs = new;
            }
            9 => { // progress_update_interval_ms (100-1000, step 50)
                let new = (self.options.progress_update_interval_ms as i64 + delta as i64 * 50)
                    .clamp(100, 1000) as u64;
                self.options.progress_update_interval_ms = new;
            }
            10 => { // verification_on_completion - toggle with +/-
                self.options.verification_on_completion = !self.options.verification_on_completion;
            }
            11 => { // concurrent_verifications (1-8, step 1)
                let new = (self.options.concurrent_verifications as i32 + delta)
                    .clamp(1, 8) as usize;
                self.options.concurrent_verifications = new;
            }
            12 => { // verification_buffer_size (64KB-512KB, step 64KB)
                let step = 64 * 1024;
                let new = (self.options.verification_buffer_size as i64 + delta as i64 * step)
                    .clamp(64 * 1024, 512 * 1024) as usize;
                self.options.verification_buffer_size = new;
            }
            13 => { // verification_update_interval (50-500, step 50)
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
            *self.status.write().unwrap() = format!("Failed to save config: {}", e);
        }
    }

    /// Navigate to next item in file tree
    pub fn next_file_tree_item(&mut self) {
        let tree = futures::executor::block_on(async {
            self.file_tree.read().unwrap().clone()
        });
        
        if let Some(tree) = tree {
            let flat = crate::ui::render::flatten_tree_for_navigation(&tree);
            let items_len = flat.len();
            
            if items_len == 0 {
                return;
            }
            
            let i = match self.file_tree_state.selected() {
                Some(i) => {
                    if i >= items_len - 1 {
                        0
                    } else {
                        i + 1
                    }
                }
                None => 0,
            };
            self.file_tree_state.select(Some(i));
        }
    }

    /// Navigate to previous item in file tree
    pub fn previous_file_tree_item(&mut self) {
        let tree = futures::executor::block_on(async {
            self.file_tree.read().unwrap().clone()
        });
        
        if let Some(tree) = tree {
            let flat = crate::ui::render::flatten_tree_for_navigation(&tree);
            let items_len = flat.len();
            
            if items_len == 0 {
                return;
            }
            
            let i = match self.file_tree_state.selected() {
                Some(i) => {
                    if i == 0 {
                        items_len - 1
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            self.file_tree_state.select(Some(i));
        }
    }

    /// Toggle expansion of directory in file tree
    pub fn toggle_file_tree_expansion(&mut self) {
        let selected_idx = match self.file_tree_state.selected() {
            Some(idx) => idx,
            None => return,
        };
        
        let mut tree = futures::executor::block_on(async {
            self.file_tree.read().unwrap().clone()
        });
        
        if let Some(ref mut tree) = tree {
            let flat = crate::ui::render::flatten_tree_for_navigation(tree);
            
            if selected_idx < flat.len() {
                let selected_path = flat[selected_idx].path.clone();
                
                // Find and toggle the node
                toggle_node_expansion(tree, &selected_path);
                
                // Update the tree
                futures::executor::block_on(async {
                    *self.file_tree.write().unwrap() = Some(tree.clone());
                });
            }
        }
    }
}

/// Helper function to toggle a node's expansion state by path
fn toggle_node_expansion(node: &mut crate::models::FileTreeNode, target_path: &str) -> bool {
    for child in &mut node.children {
        if child.path == target_path {
            if child.is_dir {
                child.expanded = !child.expanded;
            }
            return true;
        }
        
        if child.is_dir && toggle_node_expansion(child, target_path) {
            return true;
        }
    }
    false
}
