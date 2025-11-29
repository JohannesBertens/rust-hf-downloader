use crate::models::*;
use crossterm::event::EventStream;
use ratatui::widgets::ListState;
use ratatui::prelude::Rect;
use ratatui::style::Color;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tui_input::Input;
use ratatui::symbols::Marker;

// Placeholder type for download records
#[derive(Debug, Clone)]
pub struct DownloadRecord {
    pub timestamp: std::time::SystemTime,
    pub speed_mbps: f64,
    pub file_size_mb: f64,
    pub duration_secs: u64,
}

/// Enhanced canvas state for popup enhancements
#[derive(Debug, Clone, Copy)]
pub struct CanvasHoverState {
    pub in_canvas_area: bool,
    pub hover_element: Option<CanvasElement>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CanvasElement {
    SearchField,
    SearchSuggestion(usize), // Index into suggestions
    DownloadButton,
    CancelButton,
    OptionField(usize), // Field index
    PathSegment(usize), // Path segment index
    ValidationIndicator,
    // Add other interactive elements
}

#[derive(Debug, Clone)]
pub enum ValidationStatus {
    Valid,
    Invalid(String), // Error message
    Pending,
}

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
            preferred_marker: Marker::Braille,
            animation_fps: 30,
            enable_mouse_interaction: true,
            visual_feedback_level: FeedbackLevel::Standard,
        }
    }
}

/// Animation state for smooth transitions
#[derive(Debug, Clone)]
pub struct AnimationState {
    pub animations: HashMap<String, ActiveAnimation>,
    pub frame_counter: u64,
    pub target_fps: u8,
    pub frame_budget: Duration,
    pub adaptive_quality: AdaptiveQualitySettings,
}

impl Default for AnimationState {
    fn default() -> Self {
        Self {
            animations: HashMap::new(),
            frame_counter: 0,
            target_fps: 30,
            frame_budget: Duration::from_millis(33), // ~30 FPS
            adaptive_quality: AdaptiveQualitySettings::default(),
        }
    }
}

impl AnimationState {
    /// Start a new animation
    pub fn start_animation(&mut self, id: String, animation_type: AnimationType, duration: Duration) {
        let animation = ActiveAnimation {
            id: id.clone(),
            animation_type,
            start_time: std::time::Instant::now(),
            duration,
            easing_function: EasingFunction::EaseInOutCubic,
            is_paused: false,
            priority: AnimationPriority::Normal,
        };
        
        self.animations.insert(id, animation);
    }
    
    /// Update all active animations
    pub fn update(&mut self, delta_time: Duration) -> Vec<AnimationCommand> {
        let mut commands = Vec::new();
        let current_time = std::time::Instant::now();
        
        // Remove completed animations
        self.animations.retain(|_, anim| {
            current_time.duration_since(anim.start_time) < anim.duration
        });
        
        // Update active animations and generate commands
        for (_, animation) in self.animations.iter_mut() {
            if !animation.is_paused {
                let progress = (current_time.duration_since(animation.start_time).as_millis() as f64 
                    / animation.duration.as_millis() as f64).min(1.0);
                
                let eased_progress = animation.easing_function.apply(progress);
                
                match animation.animation_type {
                    AnimationType::Fade { from_alpha, to_alpha } => {
                        commands.push(AnimationCommand::SetAlpha {
                            target: animation.id.clone(),
                            alpha: from_alpha + (to_alpha - from_alpha) * eased_progress,
                        });
                    }
                    AnimationType::Slide { from_pos, to_pos } => {
                        commands.push(AnimationCommand::SetPosition {
                            target: animation.id.clone(),
                            x: from_pos.0 + (to_pos.0 - from_pos.0) * eased_progress,
                            y: from_pos.1 + (to_pos.1 - from_pos.1) * eased_progress,
                        });
                    }
                    AnimationType::Scale { from_scale, to_scale } => {
                        commands.push(AnimationCommand::SetScale {
                            target: animation.id.clone(),
                            scale: from_scale + (to_scale - from_scale) * eased_progress,
                        });
                    }
                    AnimationType::Rotate { from_angle, to_angle } => {
                        commands.push(AnimationCommand::SetRotation {
                            target: animation.id.clone(),
                            angle: from_angle + (to_angle - from_angle) * eased_progress,
                        });
                    }
                    AnimationType::Color { from_color, to_color } => {
                        commands.push(AnimationCommand::SetColor {
                            target: animation.id.clone(),
                            color: interpolate_color(from_color, to_color, eased_progress),
                        });
                    }
                }
            }
        }
        
        self.frame_counter += 1;
        commands
    }
    
    /// Pause an animation
    pub fn pause_animation(&mut self, id: &str) {
        if let Some(animation) = self.animations.get_mut(id) {
            animation.is_paused = true;
        }
    }
    
    /// Resume an animation
    pub fn resume_animation(&mut self, id: &str) {
        if let Some(animation) = self.animations.get_mut(id) {
            animation.is_paused = false;
        }
    }
    
    /// Stop an animation
    pub fn stop_animation(&mut self, id: &str) {
        self.animations.remove(id);
    }
    
    /// Get current adaptive quality level
    pub fn get_quality_level(&self) -> QualityLevel {
        self.adaptive_quality.current_level
    }
    
    /// Adjust quality based on performance
    pub fn adjust_quality(&mut self, frame_time: Duration) {
        self.adaptive_quality.adjust_quality(frame_time);
    }
}

/// Active animation instance
#[derive(Debug, Clone)]
pub struct ActiveAnimation {
    pub id: String,
    pub animation_type: AnimationType,
    pub start_time: std::time::Instant,
    pub duration: Duration,
    pub easing_function: EasingFunction,
    pub is_paused: bool,
    pub priority: AnimationPriority,
}

/// Animation types
#[derive(Debug, Clone)]
pub enum AnimationType {
    Fade { from_alpha: f64, to_alpha: f64 },
    Slide { from_pos: (f64, f64), to_pos: (f64, f64) },
    Scale { from_scale: f64, to_scale: f64 },
    Rotate { from_angle: f64, to_angle: f64 },
    Color { from_color: Color, to_color: Color },
}

/// Animation commands to be executed by renderer
#[derive(Debug, Clone)]
pub enum AnimationCommand {
    SetAlpha { target: String, alpha: f64 },
    SetPosition { target: String, x: f64, y: f64 },
    SetScale { target: String, scale: f64 },
    SetRotation { target: String, angle: f64 },
    SetColor { target: String, color: Color },
}

/// Easing functions for smooth animations
#[derive(Debug, Clone, Copy)]
pub enum EasingFunction {
    Linear,
    EaseInQuad,
    EaseOutQuad,
    EaseInOutQuad,
    EaseInCubic,
    EaseOutCubic,
    EaseInOutCubic,
    EaseInSine,
    EaseOutSine,
    EaseInOutSine,
}

impl EasingFunction {
    pub fn apply(self, t: f64) -> f64 {
        match self {
            EasingFunction::Linear => t,
            EasingFunction::EaseInQuad => t * t,
            EasingFunction::EaseOutQuad => t * (2.0 - t),
            EasingFunction::EaseInOutQuad => {
                if t < 0.5 { 2.0 * t * t } else { -1.0 + (4.0 - 2.0 * t) * t }
            }
            EasingFunction::EaseInCubic => t * t * t,
            EasingFunction::EaseOutCubic => 1.0 + (t - 1.0).powi(3),
            EasingFunction::EaseInOutCubic => {
                if t < 0.5 { 4.0 * t * t * t } else { 1.0 + (t - 1.0).powi(3) * 4.0 }
            }
            EasingFunction::EaseInSine => 1.0 - (t * std::f64::consts::PI / 2.0).cos(),
            EasingFunction::EaseOutSine => (t * std::f64::consts::PI / 2.0).sin(),
            EasingFunction::EaseInOutSine => -(std::f64::consts::PI).cos() / 2.0 + 0.5,
        }
    }
}

/// Animation priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnimationPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Adaptive quality settings
#[derive(Debug, Clone)]
pub struct AdaptiveQualitySettings {
    pub current_level: QualityLevel,
    pub performance_history: VecDeque<Duration>,
    pub adjustment_threshold: Duration,
    pub max_history_size: usize,
}

impl Default for AdaptiveQualitySettings {
    fn default() -> Self {
        Self {
            current_level: QualityLevel::High,
            performance_history: VecDeque::new(),
            adjustment_threshold: Duration::from_millis(16), // 60 FPS target
            max_history_size: 60, // Keep last 60 frames
        }
    }
}

impl AdaptiveQualitySettings {
    pub fn adjust_quality(&mut self, frame_time: Duration) {
        // Add current frame time to history
        self.performance_history.push_back(frame_time);
        if self.performance_history.len() > self.max_history_size {
            self.performance_history.pop_front();
        }
        
        // Calculate average frame time
        if self.performance_history.len() >= 10 {
            let avg_time: Duration = self.performance_history.iter().sum::<Duration>() 
                / self.performance_history.len() as u32;
            
            // Adjust quality based on performance
            if avg_time > self.adjustment_threshold * 2 {
                // Performance is bad, reduce quality
                self.current_level = match self.current_level {
                    QualityLevel::Ultra => QualityLevel::High,
                    QualityLevel::High => QualityLevel::Medium,
                    QualityLevel::Medium => QualityLevel::Low,
                    QualityLevel::Low => QualityLevel::Minimal,
                    QualityLevel::Minimal => QualityLevel::Minimal,
                };
            } else if avg_time < self.adjustment_threshold / 2 {
                // Performance is good, increase quality
                self.current_level = match self.current_level {
                    QualityLevel::Ultra => QualityLevel::Ultra,
                    QualityLevel::High => QualityLevel::Ultra,
                    QualityLevel::Medium => QualityLevel::High,
                    QualityLevel::Low => QualityLevel::Medium,
                    QualityLevel::Minimal => QualityLevel::Low,
                };
            }
        }
    }
}

/// Quality levels for rendering
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityLevel {
    Minimal,
    Low,
    Medium,
    High,
    Ultra,
}

impl QualityLevel {
    pub fn animation_framerate(self) -> u8 {
        match self {
            QualityLevel::Minimal => 15,
            QualityLevel::Low => 20,
            QualityLevel::Medium => 30,
            QualityLevel::High => 45,
            QualityLevel::Ultra => 60,
        }
    }
    
    pub fn detail_level(self) -> u8 {
        match self {
            QualityLevel::Minimal => 1,
            QualityLevel::Low => 2,
            QualityLevel::Medium => 3,
            QualityLevel::High => 4,
            QualityLevel::Ultra => 5,
        }
    }
}

/// Color interpolation helper
fn interpolate_color(from: Color, to: Color, t: f64) -> Color {
    // Simple linear interpolation - in a real implementation, you'd convert to a proper color space
    match (from, to) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            Color::Rgb(
                ((r1 as f64 + (r2 as f64 - r1 as f64) * t) as u8),
                ((g1 as f64 + (g2 as f64 - g1 as f64) * t) as u8),
                ((b1 as f64 + (b2 as f64 - b1 as f64) * t) as u8),
            )
        }
        _ => to, // Fallback for non-RGB colors
    }
}

/// Advanced canvas state for visualization and interaction
#[derive(Debug, Clone, Default)]
pub struct AdvancedCanvasState {
    pub model_visualization: ModelVisualizationState,
    pub performance_analytics: PerformanceAnalyticsState,
    pub interactive_config: InteractiveConfigState,
    pub gesture_recognition: GestureRecognitionState,
    pub animation_state: AnimationState,
    pub rendering_pipeline: CanvasRenderPipeline,
}

#[derive(Debug, Clone, Default)]
pub struct ModelVisualizationState {
    pub selected_models: Vec<usize>,
    pub comparison_mode: bool,
    pub zoom_level: f64,
    pub pan_offset: (f64, f64),
}

#[derive(Debug, Clone, Default)]
pub struct PerformanceAnalyticsState {
    pub history_data: Vec<DownloadRecord>,
    pub chart_type: ChartType,
    pub time_range: TimeRange,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum ChartType {
    #[default]
    Line,
    Bar,
    Scatter,
    Area,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum TimeRange {
    #[default]
    LastHour,
    LastDay,
    LastWeek,
    AllTime,
}

#[derive(Debug, Clone, Default)]
pub struct InteractiveConfigState {
    pub current_config: AppOptions,
    pub temp_config: AppOptions,
    pub preview_enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GestureRecognitionState {
    pub is_dragging: bool,
    pub drag_start: Option<(u16, u16)>,
    pub hover_element: Option<CanvasElement>,
}

/// Canvas rendering pipeline for performance optimization
#[derive(Debug, Clone, Default)]
pub struct CanvasRenderPipeline {
    pub dirty_rectangles: Vec<Rect>,
    pub cached_elements: HashMap<String, CanvasElement>,
    pub frame_counter: u64,
}

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
    pub advanced_canvas_state: AdvancedCanvasState,
    pub canvas_render_pipeline: CanvasRenderPipeline,
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
            // Canvas state
            canvas_marker: Marker::Braille,
            canvas_mouse_position: None,
            canvas_hover_state: CanvasHoverState {
                in_canvas_area: false,
                hover_element: None,
            },
            canvas_animation_frame: 0,
            canvas_preferences: CanvasPreferences::default(),
            advanced_canvas_state: AdvancedCanvasState::default(),
            canvas_render_pipeline: CanvasRenderPipeline::default(),
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
    
    
    
    /// Check if canvas features are available
    pub fn canvas_supported(&self) -> bool {
        // Feature detection logic
        true // Placeholder - could check terminal capabilities
    }
}
