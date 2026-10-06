//! Key handlers by input context (W3.9): [`App::handle_normal_mode_input`]
//! (the Normal-mode key map) and the five popup handlers (search,
//! options, resume, download-path, auth-error), extracted verbatim from
//! the old single-file `ui/app/events.rs`. The dispatcher
//! (`App::on_key_event`) lives in the `events` facade (`mod.rs`) and
//! routes here by `PopupMode`. Mouse handling is NOT here —
//! `handle_mouse_*`/hover/click live in `ui/app/mod.rs` next to the
//! crossterm event loop.

use crate::models::*;
use crate::ui::app::options::{OptionsFieldId, OPTIONS_FIELDS};
use crate::ui::app::state::App;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tui_input::backend::crossterm::EventHandler;

impl App {
    /// Handle keyboard input in Normal mode
    pub(super) async fn handle_normal_mode_input(&mut self, key: KeyEvent) {
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
    pub(super) async fn handle_search_popup_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.popup_mode = PopupMode::None;
                // Clear results immediately before searching
                self.clear_search_results();
                self.needs_search_models = true;
            }
            KeyCode::Esc => {
                self.popup_mode = PopupMode::None;
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
    pub(super) async fn handle_options_popup_input(&mut self, key: KeyEvent) {
        // If editing token, handle text input
        if self.options_dialog.editing_token {
            match key.code {
                KeyCode::Enter => {
                    // Save the edited token (empty string becomes None)
                    let new_token = self.options_dialog.token_input.value().to_string();
                    self.options.hf_token = if new_token.is_empty() {
                        None
                    } else {
                        Some(new_token)
                    };
                    self.options_dialog.editing_token = false;

                    // The token rides in the session's shared client
                    // (M4/B5) — rebuild it so the change takes effect
                    // for the next search/fetch (a malformed token
                    // surfaces as the error popup).
                    self.rebuild_api_client();

                    // Save to disk
                    if let Err(e) = crate::config::save_config(&self.options) {
                        *self.status.write() = format!("Failed to save config: {}", e);
                    }
                }
                KeyCode::Esc => {
                    // Cancel editing
                    self.options_dialog.editing_token = false;
                }
                _ => {
                    self.options_dialog
                        .token_input
                        .handle_event(&Event::Key(key));
                }
            }
        } else if self.options_dialog.editing_directory {
            match key.code {
                KeyCode::Enter => {
                    // Save the edited directory
                    self.options.default_directory =
                        self.options_dialog.directory_input.value().to_string();
                    self.options_dialog.editing_directory = false;

                    // Save to disk
                    if let Err(e) = crate::config::save_config(&self.options) {
                        *self.status.write() = format!("Failed to save config: {}", e);
                    }
                }
                KeyCode::Esc => {
                    // Cancel editing
                    self.options_dialog.editing_directory = false;
                }
                _ => {
                    self.options_dialog
                        .directory_input
                        .handle_event(&Event::Key(key));
                }
            }
        } else {
            // Normal navigation mode
            match key.code {
                KeyCode::Esc => {
                    self.popup_mode = PopupMode::None;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if self.options_dialog.selected_field > 0 {
                        self.options_dialog.selected_field -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    // Bound derives from the field table (W4.7): 16
                    // entries → last index 15 — the exact historical
                    // `< 15` clamp.
                    if self.options_dialog.selected_field < OPTIONS_FIELDS.len() - 1 {
                        self.options_dialog.selected_field += 1;
                    }
                }
                KeyCode::Char('+') | KeyCode::Right => {
                    self.modify_option(1);
                }
                KeyCode::Char('-') | KeyCode::Left => {
                    self.modify_option(-1);
                }
                KeyCode::Enter => {
                    // Enter edit mode for the two text fields (ids from the
                    // table — W4.7; the other 14 fields ignore Enter)
                    if let Some(spec) = OPTIONS_FIELDS.get(self.options_dialog.selected_field) {
                        match spec.id {
                            OptionsFieldId::DefaultDirectory => {
                                self.options_dialog.editing_directory = true;
                                self.options_dialog.directory_input = tui_input::Input::default()
                                    .with_value(self.options.default_directory.clone());
                            }
                            OptionsFieldId::HfToken => {
                                self.options_dialog.editing_token = true;
                                self.options_dialog.token_input = tui_input::Input::default()
                                    .with_value(
                                        self.options.hf_token.as_deref().unwrap_or("").to_string(),
                                    );
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Handle keyboard input in Resume Download popup
    pub(super) async fn handle_resume_popup_input(&mut self, key: KeyEvent) {
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
    pub(super) async fn handle_download_path_popup_input(&mut self, key: KeyEvent) {
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
    pub(super) async fn handle_auth_error_popup_input(&mut self, key: KeyEvent) {
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

    /// Check if applying a preset would change the current settings
    /// Returns true if the preset settings differ from current settings
    fn would_change_settings(&self, preset: crate::models::FilterPreset) -> bool {
        !self.filters.matches_preset(preset)
    }
}
