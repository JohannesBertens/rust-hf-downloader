use crate::models::*;
use crossterm::event::EventStream;
use ratatui::widgets::ListState;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tui_input::Input;

/// Type alias for download message tuple
/// Tuple: (model_id, filename, path, sha256, hf_token)
pub type DownloadMessage = (String, String, PathBuf, Option<String>, Option<String>);

/// Type alias for download receiver to reduce complexity
pub type DownloadReceiver = Arc<Mutex<mpsc::UnboundedReceiver<DownloadMessage>>>;

/// Main application state container
#[derive(Debug)]
pub struct App {
    pub running: bool,
    pub event_stream: EventStream,
    pub input: Input,
    pub input_mode: InputMode,
    pub focused_pane: FocusedPane,
    pub models: Arc<Mutex<Vec<ModelInfo>>>,
    pub list_state: ListState,
    pub quant_list_state: ListState,
    pub loading: bool,
    pub error: Option<String>,
    pub status: String,  // Status messages (downloads, verification, etc.)
    pub selection_info: String,  // Model selection info (name + URL)
    pub quantizations: Arc<Mutex<Vec<QuantizationGroup>>>,
    pub quant_file_list_state: ListState,
    pub loading_quants: bool,
    pub quant_cache: Arc<Mutex<QuantizationCache>>,
    pub popup_mode: PopupMode,
    pub download_path_input: Input,
    pub download_progress: Arc<Mutex<Option<DownloadProgress>>>,
    pub download_tx: mpsc::UnboundedSender<DownloadMessage>,
    pub download_rx: DownloadReceiver,
    pub download_queue_size: Arc<Mutex<usize>>,
    pub incomplete_downloads: Vec<DownloadMetadata>,
    pub status_rx: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    pub status_tx: mpsc::UnboundedSender<String>,
    pub download_registry: Arc<Mutex<DownloadRegistry>>,
    pub complete_downloads: Arc<Mutex<CompleteDownloads>>,
    pub verification_progress: Arc<Mutex<Vec<VerificationProgress>>>,
    pub verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    pub verification_queue_size: Arc<Mutex<usize>>,
    pub options: crate::models::AppOptions,
    pub options_directory_input: Input,
    pub options_token_input: Input,
    // Non-GGUF model support
    pub model_metadata: Arc<Mutex<Option<crate::models::ModelMetadata>>>,
    pub file_tree: Arc<Mutex<Option<crate::models::FileTreeNode>>>,
    pub file_tree_state: ListState,
    pub display_mode: crate::models::ModelDisplayMode,
    // Flags to trigger deferred loading on next loop iteration
    pub needs_load_quantizations: bool,
    pub needs_search_models: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Create new application instance with default state
    pub fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        
        let quant_list_state = ListState::default();
        
        let quant_file_list_state = ListState::default();
        
        let (download_tx, download_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        
        // Load options from config file (or use defaults)
        let options = crate::config::load_config();
        let mut download_path_input = Input::default();
        download_path_input = download_path_input.with_value(options.default_directory.clone());
        
        let file_tree_state = ListState::default();
        
        Self {
            running: false,
            event_stream: EventStream::default(),
            input: Input::default(),
            input_mode: InputMode::Normal,  // Start in normal mode
            focused_pane: FocusedPane::Models,
            models: Arc::new(Mutex::new(Vec::new())),
            list_state,
            quant_list_state,
            loading: false,
            error: None,
            status: "Press '/' to search, Tab to switch panes, ←/→ for sub-lists, 'd' to download, 'v' to verify, 'o' for options, 'q' to quit".to_string(),
            selection_info: String::new(),
            quantizations: Arc::new(Mutex::new(Vec::new())),
            quant_file_list_state,
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
            options_token_input: Input::default(),
            // Non-GGUF model support
            model_metadata: Arc::new(Mutex::new(None)),
            file_tree: Arc::new(Mutex::new(None)),
            file_tree_state,
            display_mode: crate::models::ModelDisplayMode::Gguf,
            needs_load_quantizations: false,
            needs_search_models: false,
        }
    }

    /// Synchronize options to global config atomics
    pub fn sync_options_to_config(&self) {
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

    /// Terminate application
    pub fn quit(&mut self) {
        self.running = false;
    }
}
