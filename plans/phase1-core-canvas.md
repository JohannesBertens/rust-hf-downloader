# Phase 1: Core Canvas Integration

## Overview
This phase establishes the foundational canvas infrastructure for the rust-hf-downloader TUI application, enabling canvas-based rendering for popup interfaces.

## Dependencies and Imports

### Required Ratatui Canvas Imports
```rust
use ratatui::{
    widgets::{
        canvas::{
            Canvas, Circle, Map, MapResolution, Points, Rectangle,
            Context, Marker, Line
        },
        Block
    },
    style::{Color, Style},
    layout::{Rect}
};
```

## Core Implementation Tasks

### 1. Canvas State Management
**File:** `src/ui/app/state.rs`
- Add canvas state to App struct:
  ```rust
  pub canvas_marker: Marker,
  pub canvas_mouse_position: Option<(u16, u16)>,
  pub canvas_hover_state: CanvasHoverState,
  pub canvas_animation_frame: u64,
  ```

**New Types:**
```rust
#[derive(Debug, Clone, Copy)]
pub struct CanvasHoverState {
    pub in_canvas_area: bool,
    pub hover_element: Option<CanvasElement>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CanvasElement {
    SearchField,
    DownloadButton,
    CancelButton,
    Option1,
    Option2,
    // Add other interactive elements
}
```

### 2. Canvas Rendering Infrastructure
**File:** `src/ui/render.rs`
- New function `render_canvas_popup()`:
  ```rust
  pub fn render_canvas_popup(
      frame: &mut Frame,
      area: Rect,
      title: &str,
      content: CanvasContent,
      marker: Marker,
  ) {
      // Calculate centered popup area
      let popup_width = 80.min(frame.area().width.saturating_sub(4));
      let popup_height = 30;
      // ... area calculations
      
      // Clear the popup area
      frame.render_widget(Clear, popup_area);
      
      // Create canvas widget
      let canvas_widget = Canvas::default()
          .block(Block::default()
              .borders(Borders::ALL)
              .title(title)
              .style(Style::default().fg(Color::Cyan)))
          .marker(marker)
          .paint(move |ctx| {
              // Canvas drawing logic here
          })
          .x_bounds([0.0, f64::from(popup_area.width)])
          .y_bounds([0.0, f64::from(popup_area.height)]);
          
      frame.render_widget(canvas_widget, popup_area);
  }
  ```

### 3. Canvas Content Types
**File:** `src/models.rs`
```rust
#[derive(Debug, Clone)]
pub enum CanvasContent {
    SearchContent {
        query: String,
        suggestions: Vec<String>,
        selected_index: usize,
    },
    DownloadContent {
        path: String,
        validation_status: ValidationStatus,
    },
    OptionsContent {
        settings: AppOptions,
        focused_field: usize,
    },
    // ... other content types
}

#[derive(Debug, Clone)]
pub enum ValidationStatus {
    Valid,
    Invalid(String),
    Pending,
}
```

### 4. Canvas Event Handling
**File:** `src/ui/app/events.rs`
- Add canvas mouse event handling:
  ```rust
  async fn handle_canvas_events(&mut self, event: MouseEvent) {
      match event.kind {
          MouseEventKind::Down(_) => {
              self.handle_canvas_click(event.column, event.row);
          }
          MouseEventKind::Moved => {
              self.update_canvas_hover(event.column, event.row);
          }
          _ => {}
      }
  }
  ```

### 5. Integration Points

#### Modify Existing Popup Renderers
Update existing `render_*_popup()` functions to optionally use canvas:
```rust
pub fn render_search_popup(frame: &mut Frame, input: &Input, use_canvas: bool) {
    if use_canvas {
        let content = CanvasContent::SearchContent {
            query: input.value().to_string(),
            suggestions: vec![], // To be implemented
            selected_index: 0,
        };
        render_canvas_popup(frame, frame.area(), " Search Models ", content, Marker::Braille);
    } else {
        // Fallback to existing paragraph-based rendering
        render_search_popup_legacy(frame, input);
    }
}
```

## Technical Specifications

### Canvas Marker Configuration
- **Default:** `Marker::Braille` (highest resolution)
- **Fallback:** `Marker::Dot` (maximum compatibility)
- **Configurable:** Allow user preference for marker type

### Performance Considerations
- **Frame Rate:** 60 FPS for animations
- **Memory Usage:** Minimize canvas buffer allocations
- **Redraw Optimization:** Only redraw changed areas

### Compatibility Requirements
- **Terminal Support:** Graceful degradation for terminals without Unicode Braille
- **Fallback:** Automatic fallback to simpler markers
- **Error Handling:** Canvas creation failures should not crash application

## Testing Strategy
1. **Unit Tests:** Canvas state management functions
2. **Integration Tests:** Canvas popup rendering with various content types
3. **Performance Tests:** Canvas rendering performance benchmarks
4. **Compatibility Tests:** Test canvas markers across different terminals

## Success Criteria
- [ ] Canvas widget successfully imported and configured
- [ ] Basic canvas popup rendering functional
- [ ] Canvas state management integrated with existing app state
- [ ] Mouse event handling for canvas areas
- [ ] Fallback rendering when canvas not supported
- [ ] Performance acceptable (< 16ms per frame)

## Next Phase Dependencies
This phase enables:
- Enhanced visual popups with canvas elements
- Interactive canvas features
- Advanced canvas animations
- Model visualization capabilities
