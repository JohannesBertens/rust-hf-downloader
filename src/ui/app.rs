use crate::api::{fetch_models, fetch_model_files, fetch_multipart_sha256s, parse_multipart_filename};
use crate::download::{start_download, validate_and_sanitize_path};
use crate::models::*;
use crate::registry;
use color_eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::{FutureExt, StreamExt};
use ratatui::{DefaultTerminal, Frame, widgets::ListState};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

#[derive(Debug)]
pub struct App {
    running: bool,
    event_stream: EventStream,
    input: Input,
    input_mode: InputMode,
    focused_pane: FocusedPane,
    models: Arc<Mutex<Vec<ModelInfo>>>,
    list_state: ListState,
    quant_list_state: ListState,
    loading: bool,
    error: Option<String>,
    status: String,  // Status messages (downloads, verification, etc.)
    selection_info: String,  // Model selection info (name + URL)
    quantizations: Arc<Mutex<Vec<QuantizationInfo>>>,
    loading_quants: bool,
    quant_cache: Arc<Mutex<QuantizationCache>>,
    popup_mode: PopupMode,
    download_path_input: Input,
    download_progress: Arc<Mutex<Option<DownloadProgress>>>,
    download_tx: mpsc::UnboundedSender<(String, String, PathBuf, Option<String>)>,
    download_rx: Arc<Mutex<mpsc::UnboundedReceiver<(String, String, PathBuf, Option<String>)>>>,
    download_queue_size: Arc<Mutex<usize>>,
    incomplete_downloads: Vec<DownloadMetadata>,
    status_rx: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    status_tx: mpsc::UnboundedSender<String>,
    download_registry: Arc<Mutex<DownloadRegistry>>,
    complete_downloads: Arc<Mutex<CompleteDownloads>>,
    verification_progress: Arc<Mutex<Vec<VerificationProgress>>>,
    verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    verification_queue_size: Arc<Mutex<usize>>,
    options: crate::models::AppOptions,
    options_directory_input: Input,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        
        let quant_list_state = ListState::default();
        
        let (download_tx, download_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        
        // Create options first to get default directory
        let options = crate::models::AppOptions::default();
        let mut download_path_input = Input::default();
        download_path_input = download_path_input.with_value(options.default_directory.clone());
        
        Self {
            running: false,
            event_stream: EventStream::default(),
            input: Input::default(),
            input_mode: InputMode::Editing,  // Start in editing mode for immediate search
            focused_pane: FocusedPane::Models,
            models: Arc::new(Mutex::new(Vec::new())),
            list_state,
            quant_list_state,
            loading: false,
            error: None,
            status: "Press '/' to search, Tab to switch lists, 'd' to download, 'v' to verify, 'o' for options, 'q' to quit".to_string(),
            selection_info: String::new(),
            quantizations: Arc::new(Mutex::new(Vec::new())),
            loading_quants: false,
            quant_cache: Arc::new(Mutex::new(HashMap::new())),
            popup_mode: PopupMode::None,
            download_path_input,
            download_progress: Arc::new(Mutex::new(None)),
            download_tx,
            download_rx: Arc::new(Mutex::new(download_rx)),
            download_queue_size: Arc::new(Mutex::new(0)),
            incomplete_downloads: Vec::new(),
            status_rx: Arc::new(Mutex::new(status_rx)),
            status_tx,
            download_registry: Arc::new(Mutex::new(DownloadRegistry::default())),
            complete_downloads: Arc::new(Mutex::new(HashMap::new())),
            verification_progress: Arc::new(Mutex::new(Vec::new())),
            verification_queue: Arc::new(Mutex::new(Vec::new())),
            verification_queue_size: Arc::new(Mutex::new(0)),
            options,
            options_directory_input: Input::default(),
        }
    }

    async fn scan_incomplete_downloads(&mut self) {
        // Load registry from disk
        let registry = registry::load_registry();
        
        // Update the app's registry
        {
            let mut reg = self.download_registry.lock().await;
            *reg = registry.clone();
        }
        
        // Find incomplete downloads
        self.incomplete_downloads = registry::get_incomplete_downloads(&registry);
        
        // Load complete downloads into memory
        let complete_map = registry::get_complete_downloads(&registry);
        
        {
            let mut complete = self.complete_downloads.lock().await;
            *complete = complete_map;
        }
        
        // Show popup if incomplete downloads found
        if !self.incomplete_downloads.is_empty() {
            self.popup_mode = PopupMode::ResumeDownload;
            self.status = format!("Found {} incomplete download(s)", self.incomplete_downloads.len());
        }
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        self.running = true;
        
        // Initialize global download config from options
        self.sync_options_to_config();
        
        // Scan for incomplete downloads on startup
        self.scan_incomplete_downloads().await;
        
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
            while let Some((model_id, filename, path, sha256)) = rx.recv().await {
                // Decrement queue size when we start processing
                {
                    let mut queue_size = download_queue_size.lock().await;
                    *queue_size = queue_size.saturating_sub(1);
                }
                start_download(
                    model_id,
                    filename,
                    path,
                    download_progress.clone(),
                    status_tx.clone(),
                    complete_downloads.clone(),
                    sha256,
                    verification_queue.clone(),
                    verification_queue_size.clone(),
                ).await;
            }
        });
        
        while self.running {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_crossterm_events().await?;
        }
        Ok(())
    }

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
        
        // Render main UI
        super::render::render_ui(
            frame,
            &self.input,
            self.input_mode,
            &models,
            &mut self.list_state,
            self.loading,
            &quantizations,
            &mut self.quant_list_state,
            self.loading_quants,
            self.focused_pane,
            &self.error,
            &self.status,
            &self.selection_info,
            &complete_downloads,
        );
        
        // Render both download and verification progress bars
        let (download_progress, download_queue_size, verification_progress, verification_queue_size) = 
            futures::executor::block_on(async {
                let dl_prog = self.download_progress.lock().await.clone();
                let dl_queue = *self.download_queue_size.lock().await;
                let ver_prog = self.verification_progress.lock().await.clone();
                let ver_queue = *self.verification_queue_size.lock().await;
                (dl_prog, dl_queue, ver_prog, ver_queue)
            });
        
        super::render::render_progress_bars(
            frame,
            &download_progress,
            download_queue_size,
            &verification_progress,
            verification_queue_size,
        );
        
        // Render popups (must be last to appear on top)
        match self.popup_mode {
            PopupMode::ResumeDownload => {
                super::render::render_resume_popup(frame, &self.incomplete_downloads);
            }
            PopupMode::DownloadPath => {
                super::render::render_download_path_popup(frame, &self.download_path_input);
            }
            PopupMode::Options => {
                super::render::render_options_popup(frame, &self.options, &self.options_directory_input);
            }
            PopupMode::None => {}
        }
    }

    async fn handle_crossterm_events(&mut self) -> Result<()> {
        // Check for status messages from download tasks
        {
            let mut rx = self.status_rx.lock().await;
            while let Ok(msg) = rx.try_recv() {
                self.status = msg;
            }
        }
        
        let delay = tokio::time::sleep(tokio::time::Duration::from_millis(100));
        tokio::select! {
            maybe_event = self.event_stream.next().fuse() => {
                match maybe_event {
                    Some(Ok(evt)) => {
                        if let Event::Key(key) = evt {
                            if key.kind == KeyEventKind::Press {
                                self.on_key_event(key).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ = delay => {
                // Timeout - just redraw
            }
        }
        Ok(())
    }

    async fn on_key_event(&mut self, key: KeyEvent) {
        self.error = None;

        // Handle popup input separately
        if self.popup_mode == PopupMode::Options {
            // If editing directory, handle text input
            if self.options.editing_directory {
                match key.code {
                    KeyCode::Enter => {
                        // Save the edited directory
                        self.options.default_directory = self.options_directory_input.value().to_string();
                        self.options.editing_directory = false;
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
                            self.options_directory_input = Input::default()
                                .with_value(self.options.default_directory.clone());
                        }
                    }
                    _ => {}
                }
            }
            return;
        } else if self.popup_mode == PopupMode::ResumeDownload {
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
            return;
        } else if self.popup_mode == PopupMode::DownloadPath {
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
            return;
        }

        match self.input_mode {
            InputMode::Normal => match (key.modifiers, key.code) {
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
            },
            InputMode::Editing => match key.code {
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
            },
        }
    }

    fn next(&mut self) {
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

    fn previous(&mut self) {
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

    fn toggle_focus(&mut self) {
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

    fn next_quant(&mut self) {
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

    fn previous_quant(&mut self) {
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

    async fn search_models(&mut self) {
        let query = self.input.value().to_string();
        
        if query.is_empty() {
            return;
        }

        self.loading = true;
        self.error = None;
        
        let models = self.models.clone();
        
        match fetch_models(&query).await {
            Ok(results) => {
                let mut models_lock = models.lock().await;
                *models_lock = results;
                self.loading = false;
                self.list_state.select(Some(0));
                self.status = format!("Found {} models", models_lock.len());
                drop(models_lock);
                
                // Load quantizations for first result
                self.load_quantizations().await;
                
                // Start background prefetch for all models
                self.start_background_prefetch();
            }
            Err(e) => {
                self.loading = false;
                self.error = Some(format!("Failed to fetch models: {}", e));
                self.status = "Search failed".to_string();
            }
        }
    }

    async fn show_model_details(&mut self) {
        let models = self.models.lock().await;
        if let Some(selected) = self.list_state.selected() {
            if selected < models.len() {
                let model = &models[selected];
                self.selection_info = format!(
                    "Selected: {} | URL: https://huggingface.co/{}",
                    model.id, model.id
                );
            }
        }
    }

    async fn show_quantization_details(&mut self) {
        let quantizations = self.quantizations.lock().await;
        if let Some(selected) = self.quant_list_state.selected() {
            if selected < quantizations.len() {
                let quant = &quantizations[selected];
                // Keep the model selection in line 1, show quant details in line 2
                self.status = format!(
                    "Type: {} | Size: {} | File: {}",
                    quant.quant_type,
                    crate::utils::format_size(quant.size),
                    quant.filename
                );
            }
        }
    }

    async fn load_quantizations(&mut self) {
        let models = self.models.lock().await;
        if let Some(selected) = self.list_state.selected() {
            if selected < models.len() {
                let model_id = models[selected].id.clone();
                drop(models);
                
                // Check cache first
                let cache = self.quant_cache.lock().await;
                if let Some(cached_quants) = cache.get(&model_id) {
                    let mut quants_lock = self.quantizations.lock().await;
                    *quants_lock = cached_quants.clone();
                    drop(cache);
                    return;
                }
                drop(cache);
                
                self.loading_quants = true;
                let quantizations = self.quantizations.clone();
                let cache = self.quant_cache.clone();
                
                match fetch_model_files(&model_id).await {
                    Ok(quants) => {
                        let mut quants_lock = quantizations.lock().await;
                        *quants_lock = quants.clone();
                        self.loading_quants = false;
                        
                        // Store in cache
                        let mut cache_lock = cache.lock().await;
                        cache_lock.insert(model_id, quants);
                    }
                    Err(_) => {
                        self.loading_quants = false;
                        let mut quants_lock = quantizations.lock().await;
                        quants_lock.clear();
                    }
                }
            }
        }
    }

    fn start_background_prefetch(&self) {
        let models = self.models.clone();
        let cache = self.quant_cache.clone();
        
        tokio::spawn(async move {
            let models_lock = models.lock().await;
            let model_list = models_lock.clone();
            drop(models_lock);
            
            for model in model_list {
                // Check if already cached
                let cache_lock = cache.lock().await;
                let already_cached = cache_lock.contains_key(&model.id);
                drop(cache_lock);
                
                if !already_cached {
                    // Fetch quantization info
                    if let Ok(quants) = fetch_model_files(&model.id).await {
                        let mut cache_lock = cache.lock().await;
                        cache_lock.insert(model.id.clone(), quants);
                    }
                    
                    // Small delay to avoid overwhelming the API
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        });
    }

    fn quit(&mut self) {
        self.running = false;
    }
    
    fn trigger_download(&mut self) {
        let quantizations = futures::executor::block_on(async {
            self.quantizations.lock().await.clone()
        });
        
        if let Some(selected) = self.quant_list_state.selected() {
            if selected < quantizations.len() {
                // Update download path input with current default directory
                self.download_path_input = Input::default()
                    .with_value(self.options.default_directory.clone());
                self.popup_mode = PopupMode::DownloadPath;
                self.status = "Enter download path and press Enter".to_string();
            }
        }
    }
    
    async fn resume_incomplete_downloads(&mut self) {
        let count = self.incomplete_downloads.len();
        
        for metadata in &self.incomplete_downloads {
            // Queue the download to resume
            let base_path = PathBuf::from(&metadata.local_path).parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(&metadata.local_path));
            
            let _ = self.download_tx.send((
                metadata.model_id.clone(),
                metadata.filename.clone(),
                base_path,
                metadata.expected_sha256.clone(),
            ));
        }
        
        // Update queue size
        {
            let mut queue_size = self.download_queue_size.lock().await;
            *queue_size += count;
        }
        
        self.status = format!("Resuming {} incomplete download(s)", count);
        self.incomplete_downloads.clear();
    }

    async fn delete_incomplete_downloads(&mut self) {
        let mut deleted = 0;
        let mut errors = Vec::new();
        
        // Load registry
        let mut registry = {
            let reg = self.download_registry.lock().await;
            reg.clone()
        };
        
        for metadata in &self.incomplete_downloads {
            // Try to delete the actual .incomplete file
            let file_path = PathBuf::from(&metadata.local_path);
            let incomplete_path = PathBuf::from(format!("{}.incomplete", file_path.display()));
            
            match tokio::fs::remove_file(&incomplete_path).await {
                Ok(_) => deleted += 1,
                Err(e) => {
                    errors.push(format!("{}: {}", metadata.filename, e));
                }
            }
            
            // Remove from registry
            registry.downloads.retain(|d| d.url != metadata.url);
        }
        
        // Save updated registry
        registry::save_registry(&registry);
        {
            let mut reg = self.download_registry.lock().await;
            *reg = registry;
        }
        
        if errors.is_empty() {
            self.status = format!("Deleted {} incomplete file(s)", deleted);
        } else {
            self.status = format!("Deleted {} file(s), {} error(s): {}", deleted, errors.len(), errors.join(", "));
        }
        self.incomplete_downloads.clear();
    }

    async fn confirm_download(&mut self) {
        let models = self.models.lock().await.clone();
        let quantizations = self.quantizations.lock().await.clone();
        
        let model_selected = self.list_state.selected();
        let quant_selected = self.quant_list_state.selected();
        
        if let (Some(model_idx), Some(quant_idx)) = (model_selected, quant_selected) {
            if model_idx < models.len() && quant_idx < quantizations.len() {
                let model = &models[model_idx];
                let quant = &quantizations[quant_idx];
                
                let base_path = self.download_path_input.value().to_string();
                
                // Validate and sanitize the path to prevent path traversal
                let model_path = match validate_and_sanitize_path(&base_path, &model.id, &quant.filename) {
                    Ok(path) => path.parent().unwrap_or(&path).to_path_buf(),
                    Err(e) => {
                        self.error = Some(format!("Invalid path: {}", e));
                        self.status = "Download cancelled due to invalid path".to_string();
                        return;
                    }
                };
                
                // Check if this is a multi-part file (e.g., "00001-of-00005.gguf")
                let files_to_download = if let Some((current_part, total_parts)) = parse_multipart_filename(&quant.filename) {
                    // Generate all part filenames
                    let mut files = Vec::new();
                    for part in 1..=total_parts {
                        let part_filename = quant.filename.replace(
                            &format!("{:05}-of-{:05}", current_part, total_parts),
                            &format!("{:05}-of-{:05}", part, total_parts)
                        );
                        files.push(part_filename);
                    }
                    files
                } else {
                    vec![quant.filename.clone()]
                };
                
                let num_files = files_to_download.len();
                
                // Fetch SHA256 hashes for all parts if this is a multi-part file
                let sha256_map = if num_files > 1 {
                    match fetch_multipart_sha256s(&model.id, &files_to_download).await {
                        Ok(map) => map,
                        Err(e) => {
                            self.status = format!("Warning: Failed to fetch SHA256 hashes: {}. Downloads will proceed without verification.", e);
                            HashMap::new()
                        }
                    }
                } else {
                    HashMap::new() // Single file uses quant.sha256 directly
                };
                
                // Load registry and add metadata entries for all files
                let mut registry = {
                    let reg = self.download_registry.lock().await;
                    reg.clone()
                };
                
                for filename in &files_to_download {
                    // Validate each filename before processing
                    let validated_path = match validate_and_sanitize_path(&base_path, &model.id, filename) {
                        Ok(path) => path,
                        Err(e) => {
                            self.error = Some(format!("Invalid filename '{}': {}", filename, e));
                            continue;
                        }
                    };
                    
                    let url = format!("https://huggingface.co/{}/resolve/main/{}", model.id, filename);
                    let local_path_str = validated_path.to_string_lossy().to_string();
                    
                    // Only add if not already in registry
                    if !registry.downloads.iter().any(|d| d.url == url) {
                        // For single files, use the SHA256 from quantization info
                        // For multi-part files, look up the hash for this specific part
                        let expected_sha256 = if num_files == 1 {
                            quant.sha256.clone()
                        } else {
                            // Look up hash for this specific part from fetched map
                            sha256_map.get(filename).and_then(|h| h.clone())
                        };
                        
                        registry.downloads.push(DownloadMetadata {
                            model_id: model.id.clone(),
                            filename: filename.clone(),
                            url: url.clone(),
                            local_path: local_path_str,
                            total_size: 0,
                            downloaded_size: 0,
                            status: DownloadStatus::Incomplete,
                            expected_sha256,
                        });
                    }
                }
                
                // Save registry with all new entries
                registry::save_registry(&registry);
                {
                    let mut reg = self.download_registry.lock().await;
                    *reg = registry;
                }
                
                // Increment queue size by number of files
                {
                    let mut queue_size = self.download_queue_size.lock().await;
                    *queue_size += num_files;
                }
                
                // Send all download requests
                let mut success_count = 0;
                for filename in &files_to_download {
                    // For single files, use the SHA256 from quantization info
                    // For multi-part files, look up the hash for this specific part
                    let sha256 = if num_files == 1 {
                        quant.sha256.clone()
                    } else {
                        // Look up hash for this specific part from already-fetched map
                        sha256_map.get(filename).and_then(|h| h.clone())
                    };
                    
                    if self.download_tx.send((
                        model.id.clone(),
                        filename.clone(),
                        model_path.clone(),
                        sha256,
                    )).is_ok() {
                        success_count += 1;
                    }
                }
                
                if success_count > 0 {
                    if num_files > 1 {
                        self.status = format!("Queued {} parts of {} to {}", num_files, quant.filename, model_path.display());
                    } else {
                        self.status = format!("Starting download of {} to {}", quant.filename, model_path.display());
                    }
                } else {
                    self.error = Some("Failed to start download".to_string());
                }
                
                // Adjust queue size if some sends failed
                if success_count < num_files {
                    let mut queue_size = self.download_queue_size.lock().await;
                    *queue_size = queue_size.saturating_sub(num_files - success_count);
                }
            }
        }
    }
    
    async fn verify_downloaded_file(&mut self) {
        let models = self.models.lock().await.clone();
        let quantizations = self.quantizations.lock().await.clone();
        let complete_downloads = self.complete_downloads.lock().await.clone();
        
        let model_selected = self.list_state.selected();
        let quant_selected = self.quant_list_state.selected();
        
        if let (Some(model_idx), Some(quant_idx)) = (model_selected, quant_selected) {
            if model_idx < models.len() && quant_idx < quantizations.len() {
                let quant = &quantizations[quant_idx];
                
                // Check if file is marked as downloaded
                if !complete_downloads.contains_key(&quant.filename) {
                    self.status = format!("File {} is not marked as downloaded", quant.filename);
                    return;
                }
                
                // Get the metadata to find local path and expected hash
                let metadata = match complete_downloads.get(&quant.filename) {
                    Some(m) => m,
                    None => {
                        self.status = format!("Could not find metadata for {}", quant.filename);
                        return;
                    }
                };
                
                // Check if we have expected hash
                let expected_hash = match &metadata.expected_sha256 {
                    Some(hash) => hash.clone(),
                    None => {
                        self.status = format!("No SHA256 hash available for {}, cannot verify", quant.filename);
                        return;
                    }
                };
                
                let local_path = std::path::PathBuf::from(&metadata.local_path);
                
                // Check if file exists
                if !local_path.exists() {
                    self.status = format!("File not found: {}", local_path.display());
                    self.error = Some(format!("File marked as downloaded but not found at {}", local_path.display()));
                    return;
                }
                
                // Get file size for progress tracking
                let file_size = match tokio::fs::metadata(&local_path).await {
                    Ok(metadata) => metadata.len(),
                    Err(_) => 0,
                };
                
                // Queue verification item (ALWAYS queue, ignoring ENABLE_DOWNLOAD_VERIFICATION)
                let item = VerificationQueueItem {
                    filename: quant.filename.clone(),
                    local_path: local_path.to_string_lossy().to_string(),
                    expected_sha256: expected_hash,
                    total_size: file_size,
                    is_manual: true,  // Mark as manual
                };
                
                crate::verification::queue_verification(
                    self.verification_queue.clone(),
                    self.verification_queue_size.clone(),
                    item,
                ).await;
                
                self.status = format!("Queued {} for verification", quant.filename);
            }
        }
    }
    
    fn modify_option(&mut self, delta: i32) {
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
    }
    
    fn sync_options_to_config(&self) {
        use std::sync::atomic::Ordering;
        
        // Download config
        crate::download::DOWNLOAD_CONFIG.concurrent_threads.store(self.options.concurrent_threads, Ordering::Relaxed);
        crate::download::DOWNLOAD_CONFIG.target_chunks.store(self.options.num_chunks, Ordering::Relaxed);
        crate::download::DOWNLOAD_CONFIG.min_chunk_size.store(self.options.min_chunk_size, Ordering::Relaxed);
        crate::download::DOWNLOAD_CONFIG.max_chunk_size.store(self.options.max_chunk_size, Ordering::Relaxed);
        crate::download::DOWNLOAD_CONFIG.enable_verification.store(self.options.verification_on_completion, Ordering::Relaxed);
        crate::download::DOWNLOAD_CONFIG.max_retries.store(self.options.max_retries, Ordering::Relaxed);
        crate::download::DOWNLOAD_CONFIG.download_timeout_secs.store(self.options.download_timeout_secs, Ordering::Relaxed);
        crate::download::DOWNLOAD_CONFIG.retry_delay_secs.store(self.options.retry_delay_secs, Ordering::Relaxed);
        crate::download::DOWNLOAD_CONFIG.progress_update_interval_ms.store(self.options.progress_update_interval_ms, Ordering::Relaxed);
        
        // Verification config
        crate::verification::VERIFICATION_CONFIG.concurrent_verifications.store(self.options.concurrent_verifications, Ordering::Relaxed);
        crate::verification::VERIFICATION_CONFIG.buffer_size.store(self.options.verification_buffer_size, Ordering::Relaxed);
        crate::verification::VERIFICATION_CONFIG.update_interval_iterations.store(self.options.verification_update_interval, Ordering::Relaxed);
    }
}
