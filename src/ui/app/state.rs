use crate::models::*;
use crossterm::event::EventStream;
use parking_lot::RwLock;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tui_input::Input;

// The queue transport types live in the engine module (single source of
// truth shared with the CLI frontend).
pub use crate::engine::{DownloadMessage, DownloadReceiver};

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
    pub status: Arc<RwLock<String>>, // Status messages (downloads, verification, etc.)
    pub selection_info: Arc<RwLock<String>>, // Model selection info (name + URL)
    pub quantizations: Arc<RwLock<Vec<QuantizationGroup>>>,
    pub quant_file_list_state: ListState,
    pub loading_quants: Arc<RwLock<bool>>,
    pub api_cache: Arc<RwLock<crate::models::ApiCache>>,
    pub popup_mode: PopupMode,
    pub download_path_input: Input,
    pub download_progress: Arc<Mutex<Option<DownloadProgress>>>,
    pub download_tx: mpsc::UnboundedSender<DownloadMessage>,
    pub download_rx: DownloadReceiver,
    pub download_queue: Arc<Mutex<crate::models::QueueState>>, // Combined queue state to reduce lock complexity
    /// Mirror of files waiting in the download channel, for HUD display
    /// (names + sizes). Same lock level as `download_queue`.
    pub download_queue_items: Arc<Mutex<Vec<crate::models::QueueItemSummary>>>,
    pub incomplete_downloads: Vec<DownloadMetadata>,
    pub status_rx: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    pub status_tx: mpsc::UnboundedSender<String>,
    pub download_registry: Arc<Mutex<DownloadRegistry>>,
    pub complete_downloads: Arc<Mutex<CompleteDownloads>>,
    pub verification_progress: Arc<Mutex<Vec<VerificationProgress>>>,
    pub verification_queue: Arc<Mutex<Vec<VerificationQueueItem>>>,
    pub verification_queue_size: Arc<AtomicUsize>,
    /// Verification tasks spawned but unfinished (engine drain signal;
    /// unread by the TUI itself, rendered through verification_progress)
    pub verification_in_flight: Arc<AtomicUsize>,
    /// Typed verification results (engine channel; the TUI does not read it,
    /// the CLI consumes it for events/exit codes). The sender half is held so
    /// the channel stays connected for the engine's lifetime.
    pub verify_tx: mpsc::UnboundedSender<VerifyOutcome>,
    pub verify_rx: Arc<Mutex<mpsc::UnboundedReceiver<VerifyOutcome>>>,
    /// Per-file download outcomes streamed by the engine manager (the TUI
    /// does not read this channel; the CLI renders live events from it).
    pub outcome_tx: mpsc::UnboundedSender<FileOutcome>,
    pub outcome_rx: Arc<Mutex<mpsc::UnboundedReceiver<FileOutcome>>>,
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
    pub cached_download_queue_items: Vec<crate::models::QueueItemSummary>,
    pub cached_verification_queue_bytes: u64,
    pub cached_verification_progress: Vec<VerificationProgress>,
    /// Session-lifetime hash verification result counters (HUD footer)
    pub verification_results: crate::verification::VerificationResultCounters,
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
        let (verify_tx, verify_rx) = mpsc::unbounded_channel();
        let (outcome_tx, outcome_rx) = mpsc::unbounded_channel();

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
            download_progress: Arc::new(Mutex::new(None)),
            download_tx,
            download_rx: Arc::new(Mutex::new(download_rx)),
            download_queue: Arc::new(Mutex::new(crate::models::QueueState::new(0, 0))),
            download_queue_items: Arc::new(Mutex::new(Vec::new())),
            incomplete_downloads: Vec::new(),
            status_rx: Arc::new(Mutex::new(status_rx)),
            status_tx,
            download_registry: Arc::new(Mutex::new(DownloadRegistry::default())),
            complete_downloads: Arc::new(Mutex::new(HashMap::new())),
            verification_progress: Arc::new(Mutex::new(Vec::new())),
            verification_queue: Arc::new(Mutex::new(Vec::new())),
            verification_queue_size: Arc::new(AtomicUsize::new(0)),
            verification_in_flight: Arc::new(AtomicUsize::new(0)),
            verify_tx,
            verify_rx: Arc::new(Mutex::new(verify_rx)),
            outcome_tx,
            outcome_rx: Arc::new(Mutex::new(outcome_rx)),
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
            cached_download_queue_items: Vec::new(),
            cached_verification_queue_bytes: 0,
            cached_verification_progress: Vec::new(),
            verification_results: crate::verification::VerificationResultCounters::default(),
        }
    }

    /// Bundle this app's shared handles into an engine state snapshot
    /// (cheap: every field is an Arc or a channel endpoint clone). Used to
    /// spawn the shared engine tasks and keeps the App the single owner of
    /// the state the renderer reads.
    pub fn engine_state(&self) -> crate::engine::EngineState {
        crate::engine::EngineState {
            download_rx: self.download_rx.clone(),
            download_queue: self.download_queue.clone(),
            download_queue_items: self.download_queue_items.clone(),
            download_progress: self.download_progress.clone(),
            complete_downloads: self.complete_downloads.clone(),
            status_tx: self.status_tx.clone(),
            status_rx: self.status_rx.clone(),
            verification_queue: self.verification_queue.clone(),
            verification_queue_size: self.verification_queue_size.clone(),
            verification_in_flight: self.verification_in_flight.clone(),
            verification_progress: self.verification_progress.clone(),
            download_registry: self.download_registry.clone(),
            verify_tx: self.verify_tx.clone(),
            verify_rx: self.verify_rx.clone(),
            outcome_tx: self.outcome_tx.clone(),
            outcome_rx: self.outcome_rx.clone(),
            verification_results: self.verification_results.clone(),
        }
    }

    /// Synchronize options to global config atomics
    pub fn sync_options_to_config(&self) {
        crate::config::apply_options(&self.options);
    }

    /// Terminate application
    pub fn quit(&mut self) {
        self.running = false;
    }
}
