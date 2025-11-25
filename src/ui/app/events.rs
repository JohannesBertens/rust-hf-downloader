use super::state::App;
use crate::models::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tui_input::backend::crossterm::EventHandler;

impl App {
    /// Main keyboard event dispatcher
    pub async fn on_key_event(&mut self, key: KeyEvent) {
        self.error = None;

        // Handle popup input separately
        if self.popup_mode == PopupMode::Options {
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
                self.input_mode = InputMode::Editing;
                self.status = "Enter search query, press Enter to search, ESC to cancel".to_string();
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
                self.status = "Press '/' to search, Tab to switch lists, 'd' to download, 'v' to verify, 'o' for options, 'q' to quit".to_string();
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
                        self.status = format!("Failed to save config: {}", e);
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
                        self.status = format!("Failed to save config: {}", e);
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
                self.status = "Skipped incomplete downloads".to_string();
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
                self.status = "Download cancelled".to_string();
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
            self.models.lock().await.len()
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
            self.models.lock().await.len()
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
        self.focused_pane = match self.display_mode {
            ModelDisplayMode::Gguf => {
                // GGUF mode: cycle Models → QuantizationGroups → QuantizationFiles → Models
                match self.focused_pane {
                    FocusedPane::Models => {
                        // When switching to quantization groups, select first item if available
                        let quants_len = futures::executor::block_on(async {
                            self.quantizations.lock().await.len()
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
                                self.quantizations.lock().await.clone()
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
                            self.file_tree.lock().await.as_ref()
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
                        self.quantizations.lock().await.clone()
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
            self.quantizations.lock().await.len()
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
            self.quantizations.lock().await.len()
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
                self.quantizations.lock().await.clone()
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
                self.quantizations.lock().await.clone()
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
            self.status = format!("Failed to save config: {}", e);
        }
    }

    /// Navigate to next item in file tree
    pub fn next_file_tree_item(&mut self) {
        let tree = futures::executor::block_on(async {
            self.file_tree.lock().await.clone()
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
            self.file_tree.lock().await.clone()
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
            self.file_tree.lock().await.clone()
        });
        
        if let Some(ref mut tree) = tree {
            let flat = crate::ui::render::flatten_tree_for_navigation(tree);
            
            if selected_idx < flat.len() {
                let selected_path = flat[selected_idx].path.clone();
                
                // Find and toggle the node
                toggle_node_expansion(tree, &selected_path);
                
                // Update the tree
                futures::executor::block_on(async {
                    *self.file_tree.lock().await = Some(tree.clone());
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
