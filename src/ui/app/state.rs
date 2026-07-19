use crate::models::*;
use crossterm::event::EventStream;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use tokio::sync::Mutex;
use tui_input::Input;

/// Main application state container
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
    pub status: Arc<RwLock<String>>, // Status messages (downloads, verification, etc.)
    pub selection_info: Arc<RwLock<String>>, // Model selection info (name + URL)
    pub quantizations: Arc<RwLock<Vec<QuantizationGroup>>>,
    pub quant_file_list_state: ListState,
    pub loading_quants: Arc<RwLock<bool>>,
    pub api_cache: Arc<RwLock<crate::models::ApiCache>>,
    pub popup_mode: PopupMode,
    pub download_path_input: Input,
    pub runtime: crate::runtime::DownloadRuntime,
    pub incomplete_downloads: Vec<DownloadMetadata>,
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
    pub focused_filter_field: usize, // 0=sort, 1=downloads, 2=likes
    // Mouse interaction state
    pub mouse_position: Option<(u16, u16)>, // Current mouse position (x, y)
    pub panel_areas: Vec<(FocusedPane, Rect)>, // Store panel areas for click/hover detection
    pub hovered_panel: Option<FocusedPane>, // Currently hovered panel for visual feedback
    pub last_mouse_event_time: std::time::Instant, // Track time of last processed mouse event
    pub filter_areas: Vec<(usize, Rect)>, // Store filter field areas (0=sort, 1=downloads, 2=likes)
    // Cached values for non-blocking render (used when tokio Mutex is locked)
    pub cached_complete_downloads: CompleteDownloads,
    pub cached_download_progress: Option<DownloadProgress>,
    pub cached_download_queue: crate::models::QueueState, // Combined cache
    pub cached_verification_progress: Vec<VerificationProgress>,
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
            input_mode: InputMode::Normal, // Start in normal mode
            focused_pane: FocusedPane::Models,
            models: Arc::new(RwLock::new(Vec::new())),
            list_state,
            quant_list_state,
            loading: Arc::new(RwLock::new(false)),
            error: Arc::new(RwLock::new(None)),
            status: Arc::new(RwLock::new(
                "Welcome! Press '/' to search for models".to_string(),
            )),
            selection_info: Arc::new(RwLock::new(String::new())),
            quantizations: Arc::new(RwLock::new(Vec::new())),
            quant_file_list_state,
            loading_quants: Arc::new(RwLock::new(false)),
            api_cache: Arc::new(RwLock::new(crate::models::ApiCache::default())),
            popup_mode: PopupMode::None,
            download_path_input,
            runtime: crate::runtime::DownloadRuntime::new(),
            incomplete_downloads: Vec::new(),
            options,
            options_directory_input: Input::default(),
            options_token_input: Input::default(),
            // Non-GGUF model support
            model_metadata: Arc::new(RwLock::new(None)),
            file_tree: Arc::new(RwLock::new(None)),
            file_tree_state,
            display_mode: Arc::new(RwLock::new(crate::models::ModelDisplayMode::Gguf)),
            needs_load_quantizations: false,
            needs_search_models: false,
            last_prefetch_time: Arc::new(Mutex::new(std::time::Instant::now())),
            sort_field: default_sort_field,
            sort_direction: default_sort_direction,
            filter_min_downloads: default_min_downloads,
            filter_min_likes: default_min_likes,
            focused_filter_field: 0,
            // Mouse interaction state
            mouse_position: None,
            panel_areas: Vec::new(),
            hovered_panel: None,
            last_mouse_event_time: std::time::Instant::now(),
            filter_areas: Vec::new(),
            // Cached values for non-blocking render
            cached_complete_downloads: HashMap::new(),
            cached_download_progress: None,
            cached_download_queue: crate::models::QueueState::new(0, 0),
            cached_verification_progress: Vec::new(),
        }
    }

    /// Synchronize options to global config atomics
    pub fn sync_options_to_config(&self) {
        use std::sync::atomic::Ordering;

        // Download config
        self.runtime.download_config
            .concurrent_threads
            .store(self.options.concurrent_threads, Ordering::Relaxed);
        self.runtime.download_config
            .target_chunks
            .store(self.options.num_chunks, Ordering::Relaxed);
        self.runtime.download_config
            .min_chunk_size
            .store(self.options.min_chunk_size, Ordering::Relaxed);
        self.runtime.download_config
            .max_chunk_size
            .store(self.options.max_chunk_size, Ordering::Relaxed);
        self.runtime.download_config
            .enable_verification
            .store(self.options.verification_on_completion, Ordering::Relaxed);
        self.runtime.download_config
            .max_retries
            .store(self.options.max_retries, Ordering::Relaxed);
        self.runtime.download_config
            .download_timeout_secs
            .store(self.options.download_timeout_secs, Ordering::Relaxed);
        self.runtime.download_config
            .retry_delay_secs
            .store(self.options.retry_delay_secs, Ordering::Relaxed);
        self.runtime.download_config
            .progress_update_interval_ms
            .store(self.options.progress_update_interval_ms, Ordering::Relaxed);

        // Rate limiting config
        let rate_limit_enabled = self.options.download_rate_limit_enabled;
        self.runtime.download_config
            .rate_limit_enabled
            .store(rate_limit_enabled, Ordering::Relaxed);
        let bytes_per_sec = (self.options.download_rate_limit_mbps * 1_048_576.0) as u64;
        self.runtime.download_config
            .rate_limit_bytes_per_sec
            .store(bytes_per_sec, Ordering::Relaxed);

        // Update rate limiter asynchronously
        let rate_limiter = self.runtime.rate_limiter.clone();
        tokio::spawn(async move {
            rate_limiter.set_rate(bytes_per_sec).await;
            rate_limiter.set_enabled(rate_limit_enabled);
        });

        // Verification config
        self.runtime.verification_config
            .concurrent_verifications
            .store(self.options.concurrent_verifications, Ordering::Relaxed);
        self.runtime.verification_config
            .buffer_size
            .store(self.options.verification_buffer_size, Ordering::Relaxed);
        self.runtime.verification_config
            .update_interval_iterations
            .store(self.options.verification_update_interval, Ordering::Relaxed);
    }

    /// Terminate application
    pub fn quit(&mut self) {
        self.running = false;
    }
}
