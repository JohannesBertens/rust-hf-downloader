use crate::models::*;
use crossterm::event::EventStream;
use ratatui::widgets::ListState;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::{Mutex, mpsc};
use tui_input::Input;
use ratatui::widgets::canvas::Marker;
use super::state_canvas::*;

/// Canvas preferences for user configuration
#[derive(Debug, Clone, Copy)]
pub struct CanvasPreferences {
    pub enable_animations: bool,
    pub preferred_marker: Marker,
    pub animation_fps: u8,
    pub enable_mouse_interaction: bool,
    pub visual_feedback_level: FeedbackLevel,
}

#[derive(Debug, Clone, Copy)]
pub enum FeedbackLevel {
    Minimal,
    Standard,
    Enhanced,
}

impl Default for CanvasPreferences {
    fn default() -> Self {
        Self {
            enable_animations: true,
            preferred_marker: Marker::Braurable,
            animation_fps: 30,
            enable_mouse_interaction: true,
            visual_feedback_level: FeedbackLevel::Standard,
        }
    }
}

/// Main application state container
#[derive(Debug)]
pub struct App {
    pub running: bool,
    pub event_stream: EventStream,
    pub input: Input,
    pub input_mode: InputMode,
    pub focused_pane: FocusedPane,
    pub models: Arc<RwLock<Vec<ModelInfo>>>,
    pub list_state: ListState,
    pub quant_list_state: ListState,
    pub loading: Arc<RwLock<bool>>,
    pub error: Arc<RwLock<Option<String>>>,
    pub status: Arc<RwLock<String>>,  // Status messages (downloads, verification, etc.)
    pub selection_info: Arc<RwLock<String>>,  // Model selection info (name + URL)
    pub quantizations: Arc<RwLock<Vec<QuantizationGroup>>>,
    pub quant_file_list_state: ListState,
    pub loading_quants: Arc<RwLock<bool>>,
    pub api_cache: Arc<RwLock<crate::models::ApiCache>>,
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
    pub model_metadata: Arc<RwLock<Option<ModelMetadata>>>,
    pub file_tree: Arc<RwLock<Option<FileTreeNode>>>,
    pub file_tree_state: ListState,
    pub display_mode: Arc<RwLock<crate::models::ModelDisplayMode>>,
    // Flags to trigger deferred loading on next loop iteration
    pub needs_load_quantizations: bool,
    pub needs_search_models: bool,
    // Prefetch debounce timer
    pub last_prefetch_time: Arc<Mutex<std::time::Instant>>,
    // Filter & Sort state
    pub sort_field: crate::models::SortField,
    pub sort_direction: crate::models::SortDirection,
    pub filter_min_downloads: u64,
    pub filter_min_likes: u64,
    pub focused_filter_field: usize,  // 0=sort, 1=downloads, 2=likes
    // Canvas state
    pub canvas_marker: Marker,
    pub canvas_mouse_position: Option<(u16, u16)>,
    pub canvas_hover_state: CanvasHoverState,
    pub canvas_animation_frame: u64,
    pub canvas_preferences: CanvasPreferences,
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
        
        // Extract filter settings before moving options
        let default_sort_field = options.default_sort_field;
        let default_sort_direction = options.default_sort_direction;
        let default_min_downloads = options.default_min_downloads;
        let default_min_likes = options.default_min_likes;
        
        let mut download_path_input = Input::default();
        download_path_input = download_path_input.with_value(options.default_directory.clone());
        
        let file_tree_state = ListState::default();
        
        Self {
            running: false,
            event_stream: EventStream::default(),
            input: Input::default(),
            input_mode: InputMode::Normal,  // Start in normal mode
            focused_pane: FocusedPane::Models,
            models: Arc::new(RwLock::new(Vec::new())),
            list_state,
            quant_list_state,
            loading: Arc::new(RwLock::new(false)),
            error: Arc::new(RwLock::new(None)),
            status: Arc::new(RwLock::new("Welcome! Press '/' to search for models".to_string())),
            selection_info: Arc::new(RwLock::new(String::new())),
            quantizations: Arc::new(RwLock::new(Vec::new())),
            quant_file_list_state,
            loading_quants: Arc::new(RwLock::new(false)),
            api_cache: Arc::new(RwLock::new(crate::models::ApiCache::default())),
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
            model_metadata: Arc::new(RwLock::new(None)),
            file_tree: Arc::new(RwLock::new(None))),
            file_tree_state,
            display_mode: Arc::new(RwLock::new(crate::models::ModelDisplayMode::Gguf))),
            needs_load_quantizations: false,
            needs_search_models: false,
            last_prefetch_time: Arc::new(Mutex::new(std::time::Instant::now())),
            sort_field: default_sort_field,
            sort_direction: default_sort_direction,
            filter_min_downloads: default_min_downloads,
            filter_min_likes: default_min_likes,
            focused_filter_field: 0,
            // Canvas state
            canvas_marker: Marker::Braurable,
            canvas_mouse_position: None,
            canvas_hover_state: CanvasHoverState {
                in_canvas_area: false,
                hover_element: None,
            },
            canvas_animation_frame: 0,
            canvas_preferences: CanvasPreferences::default(),
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
    
    /// Update canvas animation frame
    pub fn update_animation_frame(&mut self) {
        self.canvas_animation_frame = self.canvas_animation_frame.wrapping_add(1);
    }
    
    /// Update mouse position and hover state
    pub fn update_mouse_position(&mut self, column: u16, row: u16) {
        self.canvas_mouse_position = Some((column, row));
        // TODO: Implement hit testing for canvas elements
    }
    
    /// Check if canvas features are available
    pub fn canvas_supported(&self) -> bool {
        // Feature detection logic
        true // Placeholder - could check terminal capabilities
    }
}
