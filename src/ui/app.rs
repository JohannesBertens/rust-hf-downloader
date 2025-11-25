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
        
        // Set initial status and render UI once before loading models
        self.status = "Loading trending models...".to_string();
        terminal.draw(|frame| self.draw(frame))?;
        
        // Load trending models on startup
        self.load_trending_models().await;
        
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
                self.load_quantizations().await;
            }
            
            self.handle_crossterm_events().await?;
        }
        Ok(())
    }

    /// Draw UI components
    fn draw(&mut self, frame: &mut Frame) {
        // Get all the data we need for rendering
        let models = futures::executor::block_on(async {
            self.models.lock().await.clone()
        });
        
        let quantizations = futures::executor::block_on(async {
            self.quantizations.lock().await.clone()
        });
        
        let complete_downloads = futures::executor::block_on(async {
            self.complete_downloads.lock().await.clone()
        });
        
        let model_metadata = futures::executor::block_on(async {
            self.model_metadata.lock().await.clone()
        });
        
        let file_tree = futures::executor::block_on(async {
            self.file_tree.lock().await.clone()
        });
        
        // Render main UI
        crate::ui::render::render_ui(frame, crate::ui::render::RenderParams {
            input: &self.input,
            input_mode: self.input_mode,
            models: &models,
            list_state: &mut self.list_state,
            loading: self.loading,
            quantizations: &quantizations,
            quant_file_list_state: &mut self.quant_file_list_state,
            quant_list_state: &mut self.quant_list_state,
            loading_quants: self.loading_quants,
            focused_pane: self.focused_pane,
            error: &self.error,
            status: &self.status,
            selection_info: &self.selection_info,
            complete_downloads: &complete_downloads,
            display_mode: self.display_mode,
            model_metadata: &model_metadata,
            file_tree: &file_tree,
            file_tree_state: &mut self.file_tree_state,
        });
        
        // Render both download and verification progress bars
        let (download_progress, download_queue_size, verification_progress, verification_queue_size) = 
            futures::executor::block_on(async {
                let dl_prog = self.download_progress.lock().await.clone();
                let dl_queue = *self.download_queue_size.lock().await;
                let ver_prog = self.verification_progress.lock().await.clone();
                let ver_queue = *self.verification_queue_size.lock().await;
                (dl_prog, dl_queue, ver_prog, ver_queue)
            });
        
        crate::ui::render::render_progress_bars(
            frame,
            &download_progress,
            download_queue_size,
            &verification_progress,
            verification_queue_size,
        );
        
        // Render popups (must be last to appear on top)
        match self.popup_mode {
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

    /// Handle crossterm events (keyboard input, status updates)
    async fn handle_crossterm_events(&mut self) -> Result<()> {
        // Check for status messages from download tasks
        {
            let mut rx = self.status_rx.lock().await;
            while let Ok(msg) = rx.try_recv() {
                // Check for authentication error
                if let Some(model_id) = msg.strip_prefix("AUTH_ERROR:") {
                    let model_url = format!("https://huggingface.co/{}", model_id);
                    self.popup_mode = PopupMode::AuthError { model_url };
                    self.status = format!("Authentication required for {}", model_id);
                } else {
                    self.status = msg;
                }
            }
        }
        
        let delay = tokio::time::sleep(tokio::time::Duration::from_millis(100));
        tokio::select! {
            maybe_event = self.event_stream.next().fuse() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if key.kind == KeyEventKind::Press {
                        self.on_key_event(key).await;
                    }
                }
            }
            _ = delay => {
                // Timeout - just redraw
            }
        }
        Ok(())
    }
}
