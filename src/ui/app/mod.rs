// Declare submodules
mod downloads;
mod events;
mod filters;
mod search;
mod state;
mod verification;

// Re-export App struct
pub use state::App;

use crate::models::PopupMode;
use color_eyre::Result;
use crossterm::event::{Event, EventStream};
use futures::{FutureExt, StreamExt};
use ratatui::{DefaultTerminal, Frame};
use std::sync::atomic::Ordering;

impl App {
    /// Main application run loop
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        self.running = true;

        // The terminal event stream needs a live tty (crossterm's source
        // eagerly opens stdin or /dev/tty), so it is constructed here —
        // after the caller's terminal setup and immediately before its
        // first poll — instead of in `App::new`. This also keeps `App::new`
        // test-constructible in headless environments.
        let mut event_stream = EventStream::default();

        // Initialize global download config from options
        self.sync_options_to_config();

        // Scan for incomplete downloads on startup
        self.scan_incomplete_downloads().await;

        // Set initial status for empty screen
        *self.status.write() = "Welcome! Press '/' to search for models".to_string();
        terminal.draw(|frame| self.draw(frame))?;

        // Spawn the shared engine tasks (verification worker + download
        // manager). The CLI frontends bootstrap through `engine::bootstrap`
        // (fresh state → registry-mirror seed → these same spawns in this
        // order); `App::new` is sync, so the TUI composes the identical
        // pieces — the mirror seed already ran in
        // `scan_incomplete_downloads` above. The TUI keeps its download_tx
        // alive for the whole session, so the manager runs until the process
        // exits (the join handle is dropped, i.e. the task stays detached —
        // same behavior as the previous inline spawn).
        crate::engine::spawn_verification_worker(self.engine.clone());
        let _manager = crate::engine::spawn_manager(self.engine.clone());

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
                self.prefetch_adjacent_models().await;
            }

            self.handle_crossterm_events(&mut event_stream).await?;
        }
        Ok(())
    }

    /// Draw UI components
    fn draw(&mut self, frame: &mut Frame) {
        // Get all the data we need for rendering using non-blocking access
        // RwLock reads are safe and fast - use direct access
        let models = self.models.read().clone();
        let quantizations = self.quantizations.read().clone();
        let model_metadata = self.model_metadata.read().clone();
        let file_tree = self.file_tree.read().clone();

        // For tokio Mutex, snapshot() refreshes the render cache when the
        // lock is free and falls back to the cached value when the lock is
        // held by another task (the render path never blocks).
        let complete_downloads = state::snapshot(
            &self.engine.complete_downloads,
            &mut self.render_cache.complete_downloads,
        );

        // Activity HUD data is fetched BEFORE render_ui so the reserved
        // strip height is known when the main layout is split.
        let download_progress = state::snapshot(
            &self.engine.download_progress,
            &mut self.render_cache.download_progress,
        );

        let download_queue = {
            // Cache the full QueueState; project the (size, bytes) pair.
            let queue = state::snapshot(
                &self.engine.download_queue,
                &mut self.render_cache.download_queue,
            );
            (queue.size, queue.bytes)
        };

        let download_queue_items = state::snapshot(
            &self.engine.download_queue_items,
            &mut self.render_cache.download_queue_items,
        );

        let verification_progress = state::snapshot(
            &self.engine.verification_progress,
            &mut self.render_cache.verification_progress,
        );

        let verification_queue_size = self.engine.verification_queue_size.load(Ordering::Relaxed);

        // Derived variant of the snapshot pattern: the cache stores the
        // summed bytes, not a clone of the queue — the sum is computed
        // under the guard so the queue Vec is never cloned per frame.
        let verification_queue_bytes = self
            .engine
            .verification_queue
            .try_lock()
            .map(|guard| {
                let bytes = guard.iter().map(|i| i.total_size).sum();
                self.render_cache.verification_queue_bytes = bytes;
                bytes
            })
            .unwrap_or(self.render_cache.verification_queue_bytes);

        let verified_ok = self.engine.verification_results.ok.load(Ordering::Relaxed);
        let verified_fail = self
            .engine
            .verification_results
            .failed
            .load(Ordering::Relaxed);

        let hud_params = crate::ui::render::ActivityHudData {
            download_progress: &download_progress,
            queue_size: download_queue.0,
            queue_bytes: download_queue.1,
            queue_items: &download_queue_items,
            verification_progress: &verification_progress,
            verification_queue_size,
            verification_queue_bytes,
            verified_ok,
            verified_fail,
        };

        // Main UI. render_ui owns the vertical layout (W4.10): it clamps
        // the desired HUD strip height against the base layout and returns
        // the reserved strip rect plus this frame's hit-rects (W5.2: the
        // render pass is pure — the caller owns the mouse-area registry).
        let out = crate::ui::render::render_ui(
            frame,
            crate::ui::render::RenderParams {
                display_mode: *self.display_mode.read(),
                focus: crate::ui::render::FocusCtx {
                    input_mode: self.input_mode,
                    focused_pane: self.focused_pane,
                    hovered_panel: self.mouse.hovered_panel,
                },
                list: crate::ui::render::ListCtx {
                    input: &self.input,
                    models: &models,
                    list_state: &mut self.list_state,
                    loading: *self.loading.read(),
                },
                gguf: crate::ui::render::GgufPanelContext {
                    quantizations: &quantizations,
                    quant_list_state: &mut self.quant_list_state,
                    quant_file_list_state: &mut self.quant_file_list_state,
                    loading_quants: *self.loading_quants.read(),
                    complete_downloads: &complete_downloads,
                },
                standard: crate::ui::render::StandardPanelContext {
                    model_metadata: &model_metadata,
                    file_tree: &file_tree,
                    file_tree_state: &mut self.file_tree_state,
                    // Standard mode reuses the quant-loading spinner flag
                    loading: *self.loading_quants.read(),
                },
                filters: crate::ui::render::FilterCtx {
                    sort_field: self.filters.sort_field,
                    sort_direction: self.filters.sort_direction,
                    min_downloads: self.filters.min_downloads,
                    min_likes: self.filters.min_likes,
                    focused_field: self.focused_filter_field,
                },
                status: crate::ui::render::StatusCtx {
                    error: &self.error.read(),
                    status: &self.status.read(),
                    selection_info: &self.selection_info.read(),
                },
                hud_height: crate::ui::render::activity_hud_height(&hud_params),
            },
        );

        // Register this frame's hit-rects for click/hover detection
        // (replaces the former &mut Vec out-params; same registration
        // order as always — first match wins on lookup).
        self.mouse.areas = out.mouse;

        // Render the activity HUD into the strip render_ui reserved above
        // the status bar (render_ui shrank the main content accordingly)
        if out.hud_strip.height > 0 {
            crate::ui::render::render_activity_hud(frame, out.hud_strip, &hud_params);
        }

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
                crate::ui::render::render_options_popup(
                    frame,
                    &self.options,
                    &self.options_dialog,
                    &self.options_directory_input,
                    &self.options_token_input,
                );
            }
            PopupMode::AuthError { ref model_url } => {
                let has_token = self
                    .options
                    .hf_token
                    .as_ref()
                    .is_some_and(|t| !t.is_empty());
                crate::ui::render::render_auth_error_popup(frame, model_url, has_token);
            }
            PopupMode::None => {}
        }
    }

    /// Handle mouse click events immediately (synchronous)
    fn handle_mouse_click(&mut self, column: u16, row: u16) {
        // Skip if popup is open
        if self.popup_mode != crate::models::PopupMode::None {
            return;
        }

        let pos = ratatui::layout::Position::new(column, row);

        // Check if click is within any filter area first
        for (field_idx, area) in &self.mouse.areas.filters {
            if area.contains(pos) {
                self.handle_filter_click(*field_idx);
                return;
            }
        }

        // Check if click is within any panel area
        for (pane, area) in &self.mouse.areas.panels {
            if area.contains(pos) {
                // Use focus_pane() to also select first item if needed
                self.focus_pane(*pane);
                return;
            }
        }
    }

    /// Handle click on a filter field - cycle to next value
    fn handle_filter_click(&mut self, field_idx: usize) {
        // Set focused field and cycle its value (wrap-around semantics;
        // status write happens BEFORE the refresh tail, which overwrites
        // it with "Searching..." — historical order, kept)
        self.focused_filter_field = field_idx;
        self.filters.cycle(field_idx, true);
        if let Some(msg) = self.filter_status_message(field_idx) {
            *self.status.write() = msg;
        }
        self.apply_filter_refresh();
    }

    /// Status line for a filter field after a mouse cycle (None for
    /// unknown fields — the historical `_ => {}` arms wrote nothing).
    fn filter_status_message(&self, field_idx: usize) -> Option<String> {
        match field_idx {
            0 => Some(format!("Sort by: {:?}", self.filters.sort_field)),
            1 => Some(format!(
                "Min downloads: {}",
                crate::fmt::number(self.filters.min_downloads)
            )),
            2 => Some(format!(
                "Min likes: {}",
                crate::fmt::number(self.filters.min_likes)
            )),
            _ => None,
        }
    }

    /// The shared refresh tail: every filter mutation re-fetches the model
    /// list (clearing results sets "Searching..." + loading state).
    fn apply_filter_refresh(&mut self) {
        if self.filters.refresh_request() {
            self.clear_search_results();
            self.needs_search_models = true;
        }
    }

    /// Handle mouse scroll events - scroll the focused panel up or down,
    /// or cycle filter values if scrolling over filter toolbar
    fn handle_mouse_scroll(&mut self, scroll_up: bool, column: u16, row: u16) {
        // Skip if popup is open
        if self.popup_mode != crate::models::PopupMode::None {
            return;
        }

        let pos = ratatui::layout::Position::new(column, row);

        // Check if scroll is within any filter area
        for (field_idx, area) in &self.mouse.areas.filters {
            if area.contains(pos) {
                self.handle_filter_scroll(*field_idx, scroll_up);
                return;
            }
        }

        // Navigate in the currently focused pane
        match self.focused_pane {
            crate::models::FocusedPane::Models => {
                if scroll_up {
                    self.previous();
                } else {
                    self.next();
                }
                // Clear details and trigger reload (same as keyboard navigation)
                self.clear_model_details();
                self.needs_load_quantizations = true;
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

    /// Handle scroll on a filter field - cycle value up or down
    fn handle_filter_scroll(&mut self, field_idx: usize, scroll_up: bool) {
        // Set focused field and cycle (wrap-around; scroll-up = backward)
        self.focused_filter_field = field_idx;
        self.filters.cycle(field_idx, !scroll_up);
        if let Some(msg) = self.filter_status_message(field_idx) {
            *self.status.write() = msg;
        }
        self.apply_filter_refresh();
    }

    /// Update hover state based on mouse position (called once per frame with coalesced position)
    fn update_hover_state(&mut self, column: u16, row: u16) {
        // Skip if popup is open
        if self.popup_mode != crate::models::PopupMode::None {
            self.mouse.hovered_panel = None;
            return;
        }

        // Skip if no panel areas defined
        if self.mouse.areas.panels.is_empty() {
            self.mouse.hovered_panel = None;
            return;
        }

        // Find which panel (if any) the mouse is hovering over
        self.mouse.hovered_panel = self
            .mouse
            .areas
            .panels
            .iter()
            .find(|(_, area)| area.contains(ratatui::layout::Position::new(column, row)))
            .map(|(pane, _)| *pane);
    }

    /// Dispatch one terminal event (W4.8 — the select! branch and the
    /// drain loop dispatched verbatim-duplicated matches): key events go
    /// through `on_key_event` on Press only; left-clicks and scrolls act
    /// immediately with their position; mouse moves accumulate (overwrite)
    /// into `last_mouse_position` for the coalesced hover update.
    async fn process_terminal_event(
        &mut self,
        event: Event,
        last_mouse_position: &mut Option<(u16, u16)>,
    ) {
        use crossterm::event::{KeyEventKind, MouseButton, MouseEventKind};

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
                        self.handle_mouse_scroll(true, mouse_event.column, mouse_event.row);
                    }
                    MouseEventKind::ScrollDown => {
                        self.handle_mouse_scroll(false, mouse_event.column, mouse_event.row);
                    }
                    MouseEventKind::Moved => {
                        // Queue for coalesced processing — only the latest
                        // position survives
                        *last_mouse_position = Some((mouse_event.column, mouse_event.row));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Handle crossterm events with event coalescing
    /// Drains all pending events, processing keys immediately but coalescing mouse moves
    async fn handle_crossterm_events(&mut self, event_stream: &mut EventStream) -> Result<()> {
        // Check for status messages from download tasks (non-blocking)
        if let Ok(mut rx) = self.engine.status_rx.try_lock() {
            while let Ok(msg) = rx.try_recv() {
                if let Some(model_id) = crate::engine::parse_auth_status(&msg) {
                    let model_url = format!("https://huggingface.co/{}", model_id);
                    self.popup_mode = PopupMode::AuthError { model_url };
                    *self.status.write() = format!("Authentication required for {}", model_id);
                } else {
                    *self.status.write() = msg;
                }
            }
        }

        // Track the last mouse position for coalesced hover update
        let mut last_mouse_position: Option<(u16, u16)> = None;

        // Wait for at least one event or timeout
        let delay = tokio::time::sleep(tokio::time::Duration::from_millis(50));
        tokio::select! {
            maybe_event = event_stream.next().fuse() => {
                if let Some(Ok(event)) = maybe_event {
                    self.process_terminal_event(event, &mut last_mouse_position).await;
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
            match futures::poll!(event_stream.next()) {
                std::task::Poll::Ready(Some(Ok(event))) => {
                    self.process_terminal_event(event, &mut last_mouse_position)
                        .await;
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
            if self.mouse.last_move.elapsed() >= std::time::Duration::from_millis(16) {
                self.mouse.last_move = std::time::Instant::now();
                self.update_hover_state(col, row);
            }
        }

        Ok(())
    }
}
