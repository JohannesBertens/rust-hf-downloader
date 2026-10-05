use crate::models::*;
use parking_lot::RwLock;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tui_input::Input;

// The queue transport type lives in the engine module (single source of
// truth shared with the CLI frontend).
pub use crate::engine::QueuedDownload;

/// Main application state container
#[derive(Debug)]
pub struct App {
    pub running: bool,
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
    /// Tree-pane download target set by `trigger_download` when 'd' is
    /// pressed on the Standard-mode file tree: (repo path, is_directory).
    /// A directory queues every file under it. Cleared on confirm/cancel.
    pub pending_tree_download: Option<(String, bool)>,
    /// Single owned bundle of engine-owned shared state (channels, queue/
    /// registry/progress Arcs, verification counters). Every TUI access to
    /// engine state goes through explicit `self.engine.<field>` reads — no
    /// Deref, no flattened mirrors.
    pub engine: crate::engine::EngineState,
    /// Sender half of the engine's download queue channel, held by the TUI
    /// for the whole session so the manager loop runs until App drops
    /// (dropping the last clone ends the manager once the queue drains).
    /// Not part of EngineState on purpose: the engine consumes the receiver,
    /// the frontend owns the sender.
    pub download_tx: mpsc::UnboundedSender<QueuedDownload>,
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
    pub panel_areas: Vec<(FocusedPane, Rect)>, // Store panel areas for click/hover detection
    pub hovered_panel: Option<FocusedPane>,    // Currently hovered panel for visual feedback
    pub last_mouse_event_time: std::time::Instant, // Track time of last processed mouse event
    pub filter_areas: Vec<(usize, Rect)>, // Store filter field areas (0=sort, 1=downloads, 2=likes)
    // Last-known-good snapshots of the engine's tokio::Mutex state for
    // non-blocking rendering: draw() refreshes each field via `snapshot`
    // when the lock is free and falls back to the previous snapshot when
    // the lock is held by another task.
    pub render_cache: RenderCache,
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

        // One engine bundle owns every shared channel and Arc the engine
        // tasks communicate through; App keeps the download sender half to
        // enqueue work (dropping it ends the manager loop once drained).
        let (engine, download_tx) = crate::engine::EngineState::new();

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
            pending_tree_download: None,
            engine,
            download_tx,
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
            panel_areas: Vec::new(),
            hovered_panel: None,
            last_mouse_event_time: std::time::Instant::now(),
            filter_areas: Vec::new(),
            // Cached values for non-blocking render
            render_cache: RenderCache::default(),
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

/// Last-known-good snapshots of the engine's `tokio::Mutex` state, used by
/// `App::draw` for non-blocking rendering. Each field mirrors one engine
/// mutex; `draw` refreshes it through [`snapshot`] when the lock is free
/// and renders the previous snapshot when the lock is held by another
/// task. Defaults equal the engine's fresh-state initial values.
#[derive(Debug, Default)]
pub struct RenderCache {
    pub complete_downloads: CompleteDownloads,
    pub download_progress: Option<DownloadProgress>,
    /// Combined cache for the queue summary (`size`, `bytes`).
    pub download_queue: crate::models::QueueState,
    pub download_queue_items: Vec<crate::models::QueueItemSummary>,
    /// Derived under the verification-queue lock (summed `total_size`),
    /// not a clone of the queue itself.
    pub verification_queue_bytes: u64,
    pub verification_progress: Vec<VerificationProgress>,
}

/// Non-blocking snapshot of an engine `tokio::Mutex<T>` for rendering:
/// when the lock is free, copy the guarded value into `cache` and return
/// the fresh clone; when the lock is held by another task, return the
/// last-good `cache` value instead. The guard is scoped inside this
/// helper only — no lock is ever held beyond the copy (W0.8 rule).
pub(super) fn snapshot<T: Clone>(m: &Mutex<T>, cache: &mut T) -> T {
    match m.try_lock() {
        Ok(guard) => {
            *cache = guard.clone();
            guard.clone()
        }
        Err(_) => cache.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_refreshes_cache_when_lock_is_free() {
        let m = Mutex::new(vec![1u64, 2]);
        let mut cache = Vec::new();

        let got = snapshot(&m, &mut cache);

        assert_eq!(got, vec![1, 2]);
        assert_eq!(cache, vec![1, 2], "cache must be refreshed on success");
    }

    #[test]
    fn snapshot_falls_back_to_cache_when_lock_is_held() {
        let m = Mutex::new(vec![9u64]);
        let mut cache = vec![7u64];

        // Hold the lock across the call — try_lock must fail and the
        // helper must yield the cached value without touching the cache.
        let guard = m.try_lock().unwrap();
        let got = snapshot(&m, &mut cache);
        drop(guard);

        assert_eq!(got, vec![7], "held lock must yield the cached value");
        assert_eq!(cache, vec![7], "cache must be untouched on fallback");
    }

    #[test]
    fn render_cache_defaults_match_fresh_engine_state() {
        // Per-field defaults must equal the pre-RenderCache initial values
        // (HashMap::new(), None, QueueState::new(0, 0), Vec::new(), 0,
        // Vec::new()) so a fresh App renders exactly as before.
        let c = RenderCache::default();
        assert!(c.complete_downloads.is_empty());
        assert!(c.download_progress.is_none());
        assert_eq!(c.download_queue.size, 0);
        assert_eq!(c.download_queue.bytes, 0);
        assert!(c.download_queue_items.is_empty());
        assert_eq!(c.verification_queue_bytes, 0);
        assert!(c.verification_progress.is_empty());
    }
}
