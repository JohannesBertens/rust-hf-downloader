//! App state: the `App` struct, its construction, and the non-blocking
//! render cache (`RenderCache` + [`snapshot_in_place`] helper). Engine-owned state
//! lives on `App::engine` (one `EngineState`, W2.3); only TUI concerns and
//! `download_tx` are direct fields.
use crate::models::*;
use parking_lot::RwLock;
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
    // Options-dialog transient UI state
    // (docs/DEFERRED.md#options-dialog-transient-state: moved out of AppOptions —
    // cursor row + live-edit flags + the two text-edit buffers, M5/U1;
    // AppOptions is pure config schema). Lives in ui/app/options.rs.
    pub options_dialog: super::options::OptionsDialogState,
    /// The ONE `reqwest::Client` this TUI session's API requests share
    /// (plan M4/B5), built from `options.hf_token` — the token lives in
    /// the client's default `Authorization` header, so no call site
    /// handles it. Rebuilt whenever the token changes
    /// ([`App::rebuild_api_client`]); a malformed token is dropped WITH A
    /// WARNING on the status line (owner revision 2026-10-07) and the
    /// client runs unauthenticated — explicitly, never as the old silent
    /// header drop that later blamed a 401.
    pub api_client: reqwest::Client,
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
    // Filter & Sort values (cycling/stepping rules in ui/app/filters.rs;
    // focused_filter_field is focus state and stays on App)
    pub filters: super::filters::FilterState,
    pub focused_filter_field: usize, // 0=sort, 1=downloads, 2=likes
    // Mouse interaction state (one bundle — the fields travel together
    // through the render pass and the mouse handlers; see MouseState)
    pub mouse: MouseState,
    // Last-known-good snapshots of the engine's tokio::Mutex state for
    // non-blocking rendering: draw() refreshes each field via
    // `snapshot_in_place` when the lock is free and falls back to the
    // previous snapshot when
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

        // Seed filter state from the persisted config defaults before
        // `options` moves into Self (config load/save mapping unchanged —
        // AppOptions stays the serde surface, App only borrows the seeds)
        let filters = super::filters::FilterState::from_options(&options);

        let mut download_path_input = Input::default();
        download_path_input = download_path_input.with_value(options.default_directory.clone());

        let file_tree_state = ListState::default();

        // One shared API client per session (M4/B5; owner revision
        // 2026-10-07): a malformed token is DROPPED WITH A WARNING — the
        // status line carries it (not the error popup) while the client
        // runs unauthenticated; public repos stay usable and the reason
        // is on screen if a gated one 401s.
        let mut startup_error: Option<String> = None;
        let (api_client, token_warning) =
            crate::http_client::build_client_with_token(options.hf_token.as_deref(), None)
                .unwrap_or_else(|e| {
                    // TLS backend init failure is the only hard case left —
                    // same treatment as rebuild_api_client's Err arm
                    // (error field + anonymous default client).
                    startup_error = Some(format!("Failed to build HTTP client: {e}"));
                    (reqwest::Client::new(), None)
                });
        let startup_status = match (&startup_error, token_warning) {
            (Some(_), _) => "Welcome! Press '/' to search for models".to_string(),
            (None, Some(w)) => format!("Warning: Invalid HF token — {}", w.message()),
            (None, None) => "Welcome! Press '/' to search for models".to_string(),
        };

        Self {
            running: false,
            input: Input::default(),
            focused_pane: FocusedPane::Models,
            models: Arc::new(RwLock::new(Vec::new())),
            list_state,
            quant_list_state,
            loading: Arc::new(RwLock::new(false)),
            error: Arc::new(RwLock::new(startup_error)),
            status: Arc::new(RwLock::new(startup_status)),
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
            api_client,
            // Non-GGUF model support
            model_metadata: Arc::new(RwLock::new(None)),
            file_tree: Arc::new(RwLock::new(None)),
            file_tree_state,
            display_mode: Arc::new(RwLock::new(crate::models::ModelDisplayMode::Gguf)),
            needs_load_quantizations: false,
            needs_search_models: false,
            last_prefetch_time: Arc::new(Mutex::new(std::time::Instant::now())),
            filters,
            focused_filter_field: 0,
            // Mouse interaction state
            mouse: MouseState::default(),
            options_dialog: super::options::OptionsDialogState::default(),
            // Cached values for non-blocking render
            render_cache: RenderCache::default(),
        }
    }

    /// Synchronize options to global config atomics
    pub fn sync_options_to_config(&self) {
        crate::config::apply_options(&self.options);
    }

    /// Rebuild [`App::api_client`] after the token changed (options-dialog
    /// token-commit path — M4/B5; owner revision 2026-10-07). A malformed
    /// token is dropped WITH A WARNING on the status line; the client then
    /// runs unauthenticated — explicit, never the old silent header drop
    /// that later blamed a 401, and never a hard failure either.
    pub fn rebuild_api_client(&mut self) {
        match crate::http_client::build_client_with_token(self.options.hf_token.as_deref(), None) {
            Ok((client, token_warning)) => {
                self.api_client = client;
                match token_warning {
                    Some(w) => {
                        *self.status.write() =
                            format!("Warning: Invalid HF token — {}", w.message());
                    }
                    // A (now) valid token REPLACES any lingering
                    // malformed-token warning so a repaired token gets
                    // visible confirmation instead of a stale warning.
                    None => {
                        let mut status = self.status.write();
                        if status.starts_with("Warning: Invalid HF token") {
                            *status = "Token updated".to_string();
                        }
                    }
                }
            }
            Err(e) => {
                // Only the hard build failure remains an error.
                *self.error.write() = Some(format!("Failed to rebuild HTTP client: {e}"));
                self.api_client = reqwest::Client::new();
            }
        }
    }

    /// Terminate application
    pub fn quit(&mut self) {
        self.running = false;
    }
}

/// Mouse interaction state (W5.3): the per-frame hit-rect registry the
/// render pass RETURNS (stored here by `App::draw` — the render side is
/// pure) plus the hover and throttle state the event handlers maintain.
/// Hit-testing is first-match over each list; registration order is
/// behavior (see `render::MouseAreas`).
#[derive(Debug)]
pub struct MouseState {
    /// Hit-rects of the last rendered frame: clickable panel regions and
    /// filter-field regions (0=sort, 1=downloads, 2=likes).
    pub areas: crate::ui::render::MouseAreas,
    /// Panel currently under the mouse cursor, for border feedback.
    pub hovered_panel: Option<FocusedPane>,
    /// Time of the last processed mouse move; hover updates are throttled
    /// to ~60fps against it.
    pub last_move: std::time::Instant,
}

impl Default for MouseState {
    fn default() -> Self {
        Self {
            areas: crate::ui::render::MouseAreas::default(),
            hovered_panel: None,
            last_move: std::time::Instant::now(),
        }
    }
}

/// Last-known-good snapshots of the engine's `tokio::Mutex` state, used by
/// `App::draw` for non-blocking rendering. Each field mirrors one engine
/// mutex; `draw` refreshes it through [`snapshot_in_place`] when the lock is free
/// and renders the previous snapshot when the lock is held by another
/// task. Defaults equal the engine's fresh-state initial values.
#[derive(Debug, Default)]
pub struct RenderCache {
    pub complete_downloads: CompleteDownloads,
    pub download_progress: Option<DownloadProgress>,
    /// Combined cache for the queue totals summary (`size`, `bytes`).
    pub download_queue_totals: crate::models::QueueTotals,
    pub download_queue_items: Vec<crate::models::QueueItemSummary>,
    /// Derived under the verification-queue lock (summed `total_size`),
    /// not a clone of the queue itself.
    pub verification_queue_bytes: u64,
    pub verification_progress: Vec<VerificationProgress>,
}

/// Non-blocking snapshot of an engine `tokio::Mutex<T>` for rendering:
/// when the lock is free, copy the guarded value into `cache` in place;
/// when the lock is held by another task, leave `cache` unchanged.
/// The guard is scoped inside this helper only — no lock is ever held
/// beyond the copy (W0.8 rule).
pub(super) fn snapshot_in_place<T: Clone>(m: &Mutex<T>, cache: &mut T) {
    if let Ok(guard) = m.try_lock() {
        *cache = guard.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_in_place_refreshes_cache_when_lock_is_free() {
        let m = Mutex::new(vec![1u64, 2]);
        let mut cache = Vec::new();

        snapshot_in_place(&m, &mut cache);

        assert_eq!(cache, vec![1, 2], "cache must be refreshed on success");
    }

    #[test]
    fn snapshot_in_place_falls_back_to_cache_when_lock_is_held() {
        let m = Mutex::new(vec![9u64]);
        let mut cache = vec![7u64];

        // Hold the lock across the call — try_lock must fail and the
        // helper must leave the cached value untouched.
        let guard = m.try_lock().unwrap();
        snapshot_in_place(&m, &mut cache);
        drop(guard);

        assert_eq!(cache, vec![7], "cache must be untouched on fallback");
    }

    #[test]
    fn render_cache_defaults_match_fresh_engine_state() {
        // Per-field defaults must equal the pre-RenderCache initial values
        // (HashMap::new(), None, QueueTotals::new(0, 0), Vec::new(), 0,
        // Vec::new()) so a fresh App renders exactly as before.
        let c = RenderCache::default();
        assert!(c.complete_downloads.is_empty());
        assert!(c.download_progress.is_none());
        assert_eq!(c.download_queue_totals.size, 0);
        assert_eq!(c.download_queue_totals.bytes, 0);
        assert!(c.download_queue_items.is_empty());
        assert_eq!(c.verification_queue_bytes, 0);
        assert!(c.verification_progress.is_empty());
    }
}
