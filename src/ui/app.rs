// Declare submodules
mod state;
mod events;
mod models;
mod downloads;
mod verification;

// Re-export App struct
pub use state::App;

use crate::download::start_download;
use crate::models::PopupMode;
use color_eyre::Result;
use crossterm::event::{Event, KeyEventKind};
use futures::{FutureExt, StreamExt};
use ratatui::{DefaultTerminal, Frame};

impl App {
    /// Main application run loop
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        self.running = true;
        
        // Initialize global download config from options
        self.sync_options_to_config();
        
        // Scan for incomplete downloads on startup
        self.scan_incomplete_downloads().await;
        
        // Set initial status for empty screen
        *self.status.write().unwrap() = "Welcome! Press '/' to search for models".to_string();
        terminal.draw(|frame| self.draw(frame))?;
        
        // Spawn verification worker
        let verification_queue = self.verification_queue.clone();
        let verification_progress = self.verification_progress.clone();
        let verification_queue_size = self.verification_queue_size.clone();
        let status_tx_verify = self.status_tx.clone();
        let download_registry = self.download_registry.clone();
        
        tokio::spawn(async move {
            crate::verification::verification_worker(
                verification_queue,
                verification_progress,
                verification_queue_size,
                status_tx_verify,
                download_registry,
            ).await;
        });
        
        // Spawn download manager task
        let download_rx = self.download_rx.clone();
        let download_progress = self.download_progress.clone();
        let download_queue_size = self.download_queue_size.clone();
        let status_tx = self.status_tx.clone();
        let complete_downloads = self.complete_downloads.clone();
        let verification_queue = self.verification_queue.clone();
        let verification_queue_size = self.verification_queue_size.clone();
        tokio::spawn(async move {
            let mut rx = download_rx.lock().await;
            while let Some((model_id, filename, path, sha256, hf_token)) = rx.recv().await {
                // Decrement queue size when we start processing
                {
                    let mut queue_size = download_queue_size.lock().await;
                    *queue_size = queue_size.saturating_sub(1);
                }
                start_download(crate::download::DownloadParams {
                    model_id,
                    filename,
                    base_path: path,
                    progress: download_progress.clone(),
                    status_tx: status_tx.clone(),
                    complete_downloads: complete_downloads.clone(),
                    expected_sha256: sha256,
                    verification_queue: verification_queue.clone(),
                    verification_queue_size: verification_queue_size.clone(),
                    hf_token,
                }).await;
            }
        });
        
        while self.running {
            terminal.draw(|frame| self.draw(frame))?;
            
            // Check if we need to search for models after UI render
            if self.needs_search_models {
                self.needs_search_models = false;
                self.search_models().await;
            }
            
            // Check if we need to load quantizations after UI render
            if self.needs_load_quantizations {
                self.needs_load_quantizations = false;
                self.spawn_load_quantizations();
                self.prefetch_adjacent_models();
            }
            
            self.handle_crossterm_events().await?;
        }
        Ok(())
    }

    /// Draw UI components
    fn draw(&mut self, frame: &mut Frame) {
        // Get all the data we need for rendering using non-blocking access
        // RwLock reads are safe and fast - use direct access
        let models = self.models.read().unwrap().clone();
        let quantizations = self.quantizations.read().unwrap().clone();
        let model_metadata = self.model_metadata.read().unwrap().clone();
        let file_tree = self.file_tree.read().unwrap().clone();
        
        // For tokio Mutex, use try_lock() to avoid blocking/deadlock
        // Fall back to cached values if lock is held by another task
        let complete_downloads = self.complete_downloads.try_lock()
            .map(|guard| {
                // Update cache when we successfully get the lock
                self.cached_complete_downloads = guard.clone();
                guard.clone()
            })
            .unwrap_or_else(|_| self.cached_complete_downloads.clone());
        
        // Render main UI
        crate::ui::render::render_ui(frame, crate::ui::render::RenderParams {
            input: &self.input,
            input_mode: self.input_mode,
            models: &models,
            list_state: &mut self.list_state,
            loading: *self.loading.read().unwrap(),
            quantizations: &quantizations,
            quant_file_list_state: &mut self.quant_file_list_state,
            quant_list_state: &mut self.quant_list_state,
            loading_quants: *self.loading_quants.read().unwrap(),
            focused_pane: self.focused_pane,
            error: &self.error.read().unwrap(),
            status: &self.status.read().unwrap(),
            selection_info: &self.selection_info.read().unwrap(),
            complete_downloads: &complete_downloads,
            display_mode: *self.display_mode.read().unwrap(),
            model_metadata: &model_metadata,
            file_tree: &file_tree,
            file_tree_state: &mut self.file_tree_state,
            sort_field: self.sort_field,
            sort_direction: self.sort_direction,
            filter_min_downloads: self.filter_min_downloads,
            filter_min_likes: self.filter_min_likes,
            focused_filter_field: self.focused_filter_field,
            panel_areas: &mut self.panel_areas,
            hovered_panel: &self.hovered_panel,
        });
        
        // For progress bars, use try_lock() with fallback to cached values
        let download_progress = self.download_progress.try_lock()
            .map(|guard| {
                self.cached_download_progress = guard.clone();
                guard.clone()
            })
            .unwrap_or_else(|_| self.cached_download_progress.clone());
        
        let download_queue_size = self.download_queue_size.try_lock()
            .map(|guard| {
                self.cached_download_queue_size = *guard;
                *guard
            })
            .unwrap_or(self.cached_download_queue_size);
        
        let verification_progress = self.verification_progress.try_lock()
            .map(|guard| {
                self.cached_verification_progress = guard.clone();
                guard.clone()
            })
            .unwrap_or_else(|_| self.cached_verification_progress.clone());
        
        let verification_queue_size = self.verification_queue_size.try_lock()
            .map(|guard| {
                self.cached_verification_queue_size = *guard;
                *guard
            })
            .unwrap_or(self.cached_verification_queue_size);
        
        crate::ui::render::render_progress_bars(
            frame,
            &download_progress,
            download_queue_size,
            &verification_progress,
            verification_queue_size,
        );
        
        // Render popups (must be last to appear on top)
        match self.popup_mode {
            PopupMode::SearchPopup => {
                crate::ui::render::render_search_popup(frame, &self.input);
            }
            PopupMode::ResumeDownload => {
                crate::ui::render::render_resume_popup(frame, &self.incomplete_downloads);
            }
            PopupMode::DownloadPath => {
                crate::ui::render::render_download_path_popup(frame, &self.download_path_input);
            }
            PopupMode::Options => {
                crate::ui::render::render_options_popup(frame, &self.options, &self.options_directory_input, &self.options_token_input);
            }
            PopupMode::AuthError { ref model_url } => {
                let has_token = self.options.hf_token.as_ref().is_some_and(|t| !t.is_empty());
                crate::ui::render::render_auth_error_popup(frame, model_url, has_token);
            }
            PopupMode::None => {}
        }
    }

    /// Handle mouse click events immediately (synchronous)
    fn handle_mouse_click(&mut self, column: u16, row: u16) {
        // Skip if popup is open or no panel areas defined
        if self.popup_mode != crate::models::PopupMode::None || self.panel_areas.is_empty() {
            return;
        }
        
        // Check if click is within any panel area
        for (pane, area) in &self.panel_areas {
            if area.contains(ratatui::layout::Position::new(column, row)) {
                // Use focus_pane() to also select first item if needed
                self.focus_pane(*pane);
                break;
            }
        }
    }

    /// Handle mouse scroll events - scroll the focused panel up or down
    fn handle_mouse_scroll(&mut self, scroll_up: bool) {
        // Skip if popup is open
        if self.popup_mode != crate::models::PopupMode::None {
            return;
        }
        
        // Navigate in the currently focused pane
        match self.focused_pane {
            crate::models::FocusedPane::Models => {
                if scroll_up {
                    self.previous();
                } else {
                    self.next();
                }
            }
            crate::models::FocusedPane::QuantizationGroups => {
                if scroll_up {
                    self.previous_quant();
                } else {
                    self.next_quant();
                }
            }
            crate::models::FocusedPane::QuantizationFiles => {
                if scroll_up {
                    self.previous_file();
                } else {
                    self.next_file();
                }
            }
            crate::models::FocusedPane::ModelMetadata => {
                // Metadata pane has no scrollable list
            }
            crate::models::FocusedPane::FileTree => {
                if scroll_up {
                    self.previous_file_tree_item();
                } else {
                    self.next_file_tree_item();
                }
            }
        }
    }

    /// Update hover state based on mouse position (called once per frame with coalesced position)
    fn update_hover_state(&mut self, column: u16, row: u16) {
        self.mouse_position = Some((column, row));
        
        // Skip if popup is open
        if self.popup_mode != crate::models::PopupMode::None {
            self.hovered_panel = None;
            return;
        }
        
        // Skip if no panel areas defined
        if self.panel_areas.is_empty() {
            self.hovered_panel = None;
            return;
        }
        
        // Find which panel (if any) the mouse is hovering over
        self.hovered_panel = self.panel_areas.iter()
            .find(|(_, area)| area.contains(ratatui::layout::Position::new(column, row)))
            .map(|(pane, _)| *pane);
    }

    /// Handle crossterm events with event coalescing
    /// Drains all pending events, processing keys immediately but coalescing mouse moves
    async fn handle_crossterm_events(&mut self) -> Result<()> {
        use crossterm::event::{MouseEventKind, MouseButton};
        
        // Check for status messages from download tasks (non-blocking)
        if let Ok(mut rx) = self.status_rx.try_lock() {
            while let Ok(msg) = rx.try_recv() {
                if let Some(model_id) = msg.strip_prefix("AUTH_ERROR:") {
                    let model_url = format!("https://huggingface.co/{}", model_id);
                    self.popup_mode = PopupMode::AuthError { model_url };
                    *self.status.write().unwrap() = format!("Authentication required for {}", model_id);
                } else {
                    *self.status.write().unwrap() = msg;
                }
            }
        }
        
        // Track the last mouse position for coalesced hover update
        let mut last_mouse_position: Option<(u16, u16)> = None;
        
        // Wait for at least one event or timeout
        let delay = tokio::time::sleep(tokio::time::Duration::from_millis(50));
        tokio::select! {
            maybe_event = self.event_stream.next().fuse() => {
                if let Some(Ok(event)) = maybe_event {
                    match event {
                        Event::Key(key) => {
                            if key.kind == KeyEventKind::Press {
                                self.on_key_event(key).await;
                            }
                        }
                        Event::Mouse(mouse_event) => {
                            match mouse_event.kind {
                                MouseEventKind::Down(MouseButton::Left) => {
                                    // Process clicks immediately
                                    self.handle_mouse_click(mouse_event.column, mouse_event.row);
                                }
                                MouseEventKind::ScrollUp => {
                                    // Process scroll immediately
                                    self.handle_mouse_scroll(true);
                                }
                                MouseEventKind::ScrollDown => {
                                    // Process scroll immediately
                                    self.handle_mouse_scroll(false);
                                }
                                MouseEventKind::Moved => {
                                    // Queue for coalesced processing
                                    last_mouse_position = Some((mouse_event.column, mouse_event.row));
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ = delay => {
                // Timeout - just proceed to drain any pending events
            }
        }
        
        // Drain any additional pending events without blocking
        // This coalesces multiple mouse move events into one
        loop {
            // Use poll to check if there are more events without blocking
            use futures::stream::StreamExt;
            match futures::poll!(self.event_stream.next()) {
                std::task::Poll::Ready(Some(Ok(event))) => {
                    match event {
                        Event::Key(key) => {
                            if key.kind == KeyEventKind::Press {
                                self.on_key_event(key).await;
                            }
                        }
                        Event::Mouse(mouse_event) => {
                            match mouse_event.kind {
                                MouseEventKind::Down(MouseButton::Left) => {
                                    self.handle_mouse_click(mouse_event.column, mouse_event.row);
                                }
                                MouseEventKind::ScrollUp => {
                                    self.handle_mouse_scroll(true);
                                }
                                MouseEventKind::ScrollDown => {
                                    self.handle_mouse_scroll(false);
                                }
                                MouseEventKind::Moved => {
                                    // Overwrite - only keep the latest position
                                    last_mouse_position = Some((mouse_event.column, mouse_event.row));
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                std::task::Poll::Ready(Some(Err(_))) => {
                    // Error reading event, skip
                    continue;
                }
                std::task::Poll::Ready(None) | std::task::Poll::Pending => {
                    // No more events or stream ended
                    break;
                }
            }
        }
        
        // Apply coalesced hover update once (if mouse moved)
        if let Some((col, row)) = last_mouse_position {
            // Throttle hover updates to ~60fps
            if self.last_mouse_event_time.elapsed() >= std::time::Duration::from_millis(16) {
                self.last_mouse_event_time = std::time::Instant::now();
                self.update_hover_state(col, row);
            }
        }
        
        Ok(())
    }
}
