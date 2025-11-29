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

#[derive(Debug, Clone, Copy)]
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
