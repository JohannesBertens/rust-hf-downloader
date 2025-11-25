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
                if self.focused_pane == FocusedPane::Quantizations {
                    self.trigger_download();
                }
            }
            (_, KeyCode::Char('v')) => {
                if self.focused_pane == FocusedPane::Quantizations {
                    self.verify_downloaded_file().await;
                }
            }
            (_, KeyCode::Char('o')) => {
                self.popup_mode = PopupMode::Options;
            }
            (_, KeyCode::Tab) => {
                self.toggle_focus();
            }
            (_, KeyCode::Down | KeyCode::Char('j')) => {
                match self.focused_pane {
                    FocusedPane::Models => {
                        self.next();
                        self.load_quantizations().await;
                    }
                    FocusedPane::Quantizations => {
                        self.next_quant();
                    }
                }
            }
            (_, KeyCode::Up | KeyCode::Char('k')) => {
                match self.focused_pane {
                    FocusedPane::Models => {
                        self.previous();
                        self.load_quantizations().await;
                    }
                    FocusedPane::Quantizations => {
                        self.previous_quant();
                    }
                }
            }
            (_, KeyCode::Enter) => {
                match self.focused_pane {
                    FocusedPane::Models => {
                        // Switch focus to Quantizations list
                        self.toggle_focus();
                        self.show_model_details().await;
                    }
                    FocusedPane::Quantizations => {
                        self.show_quantization_details().await;
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
                self.status = "Searching...".to_string();
                self.search_models().await;
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
        // If editing directory, handle text input
        if self.options.editing_directory {
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
                    if self.options.selected_field < 12 {
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
                    // Enter edit mode for directory field
                    if self.options.selected_field == 0 {
                        self.options.editing_directory = true;
                        self.options_directory_input = tui_input::Input::default()
                            .with_value(self.options.default_directory.clone());
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

    /// Toggle focus between Models and Quantizations panes
    pub fn toggle_focus(&mut self) {
        self.focused_pane = match self.focused_pane {
            FocusedPane::Models => {
                // When switching to quantizations, select first item if available
                let quants_len = futures::executor::block_on(async {
                    self.quantizations.lock().await.len()
                });
                if quants_len > 0 {
                    self.quant_list_state.select(Some(0));
                }
                FocusedPane::Quantizations
            }
            FocusedPane::Quantizations => FocusedPane::Models,
        };
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

    /// Modify option value based on selected field and delta
    pub fn modify_option(&mut self, delta: i32) {
        match self.options.selected_field {
            0 => {} // default_directory - use Enter to edit
            1 => { // concurrent_threads (1-32)
                let new = (self.options.concurrent_threads as i32 + delta)
                    .clamp(1, 32) as usize;
                self.options.concurrent_threads = new;
            }
            2 => { // num_chunks (10-100)
                let new = (self.options.num_chunks as i32 + delta)
                    .clamp(10, 100) as usize;
                self.options.num_chunks = new;
            }
            3 => { // min_chunk_size (1MB-50MB)
                let step = 1024 * 1024; // 1MB
                let new = (self.options.min_chunk_size as i64 + delta as i64 * step)
                    .clamp(1024 * 1024, 50 * 1024 * 1024) as u64;
                self.options.min_chunk_size = new;
            }
            4 => { // max_chunk_size (10MB-500MB)
                let step = 10 * 1024 * 1024; // 10MB
                let new = (self.options.max_chunk_size as i64 + delta as i64 * step)
                    .clamp(10 * 1024 * 1024, 500 * 1024 * 1024) as u64;
                self.options.max_chunk_size = new;
            }
            5 => { // max_retries (0-10, step 1)
                let new = (self.options.max_retries as i32 + delta)
                    .clamp(0, 10) as u32;
                self.options.max_retries = new;
            }
            6 => { // download_timeout_secs (60-600, step 30)
                let new = (self.options.download_timeout_secs as i64 + delta as i64 * 30)
                    .clamp(60, 600) as u64;
                self.options.download_timeout_secs = new;
            }
            7 => { // retry_delay_secs (1-10, step 1)
                let new = (self.options.retry_delay_secs as i64 + delta as i64)
                    .clamp(1, 10) as u64;
                self.options.retry_delay_secs = new;
            }
            8 => { // progress_update_interval_ms (100-1000, step 50)
                let new = (self.options.progress_update_interval_ms as i64 + delta as i64 * 50)
                    .clamp(100, 1000) as u64;
                self.options.progress_update_interval_ms = new;
            }
            9 => { // verification_on_completion - toggle with +/-
                self.options.verification_on_completion = !self.options.verification_on_completion;
            }
            10 => { // concurrent_verifications (1-8, step 1)
                let new = (self.options.concurrent_verifications as i32 + delta)
                    .clamp(1, 8) as usize;
                self.options.concurrent_verifications = new;
            }
            11 => { // verification_buffer_size (64KB-512KB, step 64KB)
                let step = 64 * 1024;
                let new = (self.options.verification_buffer_size as i64 + delta as i64 * step)
                    .clamp(64 * 1024, 512 * 1024) as usize;
                self.options.verification_buffer_size = new;
            }
            12 => { // verification_update_interval (50-500, step 50)
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
}
