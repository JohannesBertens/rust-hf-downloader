# Phase 2: Enhanced Visual Popups

## Overview
This phase enhances the existing popup interfaces with canvas-based visual elements, adding animations, interactive feedback, and improved information display while maintaining full backward compatibility.

## Enhanced Popup Implementations

### 1. Search Popup Enhancement

**File:** `src/ui/render.rs`

#### Animated Search Indicator
```rust
fn render_canvas_search_indicator(ctx: &mut Context, width: f64, height: f64, progress: f64) {
    // Animated search spinner using canvas circles
    let center_x = width / 2.0;
    let center_y = height / 2.0;
    let radius = 10.0;
    
    // Draw spinning indicator
    for i in 0..12 {
        let angle = (i as f64 / 12.0) * std::f64::consts::PI * 2.0 + progress;
        let x = center_x + radius * angle.cos();
        let y = center_y + radius * angle.sin();
        
        ctx.draw(&Circle {
            x,
            y,
            radius: 1.5,
            color: if i as f64 < progress * 12.0 { Color::Cyan } else { Color::DarkGray },
        });
    }
}
```

#### Search Results Preview with Canvas
```rust
fn render_canvas_search_preview(
    ctx: &mut Context,
    suggestions: &[String],
    selected_index: usize,
    area: Rect,
) {
    let line_height = 2.0;
    
    suggestions.iter().enumerate().for_each(|(i, suggestion)| {
        let y = 5.0 + (i as f64 * line_height);
        let is_selected = i == selected_index;
        
        // Background rectangle for selection
        ctx.draw(&Rectangle {
            x: 2.0,
            y,
            width: area.width as f64 - 4.0,
            height: line_height,
            color: if is_selected { Color::Blue } else { Color::Black },
        });
        
        // Draw suggestion text (simplified - full text rendering would need font support)
        ctx.print(4.0, y + 1.0, suggestion.chars().take(20).collect::<String>());
        
        // Selection indicator
        if is_selected {
            ctx.print(1.0, y + 1.0, "▶");
        }
    });
}
```

### 2. Download Path Popup Enhancement

**File:** `src/ui/render.rs`

#### Directory Tree Visualization
```rust
fn render_canvas_directory_tree(
    ctx: &mut Context,
    path_components: &[String],
    current_index: usize,
    area: Rect,
) {
    let indent_width = 15.0;
    let line_height = 2.0;
    
    path_components.iter().enumerate().for_each(|(i, component)| {
        let y = 5.0 + (i as f64 * line_height);
        let x = 2.0 + (i as f64 * indent_width);
        let is_current = i == current_index;
        
        // Draw folder/directory icon using canvas shapes
        if i < path_components.len() - 1 {
            // Draw folder shape
            ctx.draw(&Rectangle {
                x,
                y,
                width: 12.0,
                height: 8.0,
                color: if is_current { Color::Yellow } else { Color::Blue },
            });
        }
        
        // Draw component text
        ctx.print(x + 14.0, y + 1.0, component);
        
        // Draw path indicator line
        if i < path_components.len() - 1 {
            ctx.draw(&Line::new(
                x + 6.0,
                y + 8.0,
                x + 6.0,
                y + 8.0 + line_height,
                Color::DarkGray,
            ));
        }
    });
}
```

#### Path Validation Visual Feedback
```rust
fn render_path_validation_feedback(
    ctx: &mut Context,
    validation_status: &ValidationStatus,
    area: Rect,
) {
    let center_x = area.width as f64 / 2.0;
    let center_y = area.height as f64 - 8.0;
    
    match validation_status {
        ValidationStatus::Valid => {
            // Green checkmark
            ctx.draw(&Circle {
                x: center_x,
                y: center_y,
                radius: 3.0,
                color: Color::Green,
            });
            ctx.print(center_x - 2.0, center_y, "✓");
        }
        ValidationStatus::Invalid(_) => {
            // Red X
            ctx.draw(&Circle {
                x: center_x,
                y: center_y,
                radius: 3.0,
                color: Color::Red,
            });
            ctx.print(center_x - 2.0, center_y, "✗");
        }
        ValidationStatus::Pending => {
            // Yellow dots
            for i in 0..3 {
                ctx.draw(&Circle {
                    x: center_x - 6.0 + (i as f64 * 6.0),
                    y: center_y,
                    radius: 1.5,
                    color: Color::Yellow,
                });
            }
        }
    }
}
```

### 3. Options Popup Enhancement

**File:** `src/ui/render.rs`

#### Configuration Visualization with Canvas Charts
```rust
fn render_canvas_options_chart(
    ctx: &mut Context,
    settings: &AppOptions,
    focused_field: usize,
    area: Rect,
) {
    // Render horizontal bar charts for numeric settings
    let chart_height = 4.0;
    let chart_width = area.width as f64 - 20.0;
    let start_y = 5.0;
    
    // Thread count visualization
    let thread_percentage = (settings.concurrent_threads as f64 / 16.0) * 100.0;
    ctx.draw(&Rectangle {
        x: 10.0,
        y: start_y,
        width: (chart_width * thread_percentage / 100.0),
        height: chart_height,
        color: if focused_field == 0 { Color::Cyan } else { Color::Blue },
    });
    
    // Chunk size visualization
    let chunk_percentage = ((settings.max_chunk_size / 1024 / 1024) as f64 / 64.0) * 100.0;
    ctx.draw(&Rectangle {
        x: 10.0,
        y: start_y + chart_height + 2.0,
        width: (chart_width * chunk_percentage / 100.0),
        height: chart_height,
        color: if focused_field == 1 { Color::Cyan } else { Color::Blue },
    });
    
    // Verification toggle visualization
    let verification_color = if settings.verification_on_completion {
        Color::Green
    } else {
        Color::DarkGray
    };
    ctx.draw(&Circle {
        x: 15.0,
        y: start_y + (chart_height + 2.0) * 2.0,
        radius: 3.0,
        color: if focused_field == 2 { Color::Yellow } else { verification_color },
    });
}
```

#### Settings Comparison Display
```rust
fn render_settings_comparison(
    ctx: &mut Context,
    current: &AppOptions,
    default: &AppOptions,
    area: Rect,
) {
    let comparison_items = vec![
        ("Threads", current.concurrent_threads, default.concurrent_threads),
        ("Timeout", current.download_timeout_secs, default.download_timeout_secs),
        ("Retries", current.max_retries, default.max_retries),
    ];
    
    comparison_items.iter().enumerate().for_each(|(i, (label, current_val, default_val))| {
        let y = 5.0 + (i as f64 * 3.0);
        
        // Draw comparison bars
        let current_bar_width = (*current_val as f64 / 100.0) * 30.0;
        let default_bar_width = (*default_val as f64 / 100.0) * 30.0;
        
        ctx.draw(&Rectangle {
            x: 5.0,
            y,
            width: current_bar_width,
            height: 2.0,
            color: Color::Cyan,
        });
        
        ctx.draw(&Rectangle {
            x: 5.0 + 32.0,
            y,
            width: default_bar_width,
            height: 2.0,
            color: Color::DarkGray,
        });
        
        // Draw labels
        ctx.print(0.0, y, label);
        ctx.print(40.0, y, &current_val.to_string());
    });
}
```

## Interactive Features

### Mouse Interaction Handlers
**File:** `src/ui/app/events.rs`

```rust
async fn handle_canvas_popup_input(&mut self, event: MouseEvent) {
    match event.kind {
        MouseEventKind::Down(_) => {
            let click_pos = (event.column, event.row);
            self.handle_canvas_popup_click(click_pos);
        }
        MouseEventKind::Moved => {
            let hover_pos = (event.column, event.row);
            self.update_popup_hover(hover_pos);
        }
        _ => {}
    }
}
```

### Keyboard Navigation Enhancement
```rust
fn handle_canvas_popup_navigation(&mut self, key: KeyEvent) {
    match key.code {
        KeyCode::Tab => {
            // Navigate between canvas elements
            self.popup_focus_index = (self.popup_focus_index + 1) % self.canvas_element_count;
        }
        KeyCode::Enter => {
            // Activate focused canvas element
            self.activate_canvas_element(self.popup_focus_index);
        }
        _ => {}
    }
}
```

## State Management Extensions

### Enhanced App State
**File:** `src/ui/app/state.rs`

```rust
pub struct CanvasPopupState {
    pub animation_frame: u64,
    pub hover_element: Option<usize>,
    pub focus_index: usize,
    pub search_progress: f64,
    pub path_validation: ValidationStatus,
    pub options_comparison: bool,
}

impl Default for CanvasPopupState {
    fn default() -> Self {
        Self {
            animation_frame: 0,
            hover_element: None,
            focus_index: 0,
            search_progress: 0.0,
            path_validation: ValidationStatus::Pending,
            options_comparison: false,
        }
    }
}
```

## Performance Optimization

### Canvas Rendering Optimization
- **Batch Drawing:** Combine similar canvas operations
- **Dirty Rectangles:** Only redraw changed areas
- **Frame Rate Control:** Limit animation updates to 30 FPS
- **Memory Pooling:** Reuse canvas context objects

### Fallback Strategy
- **Feature Detection:** Check terminal canvas support
- **Progressive Enhancement:** Enable canvas features when available
- **Graceful Degradation:** Fallback to text-based rendering

## Testing Strategy
1. **Visual Regression Tests:** Canvas rendering consistency
2. **Interactive Tests:** Mouse and keyboard interaction
3. **Performance Tests:** Canvas rendering performance
4. **Accessibility Tests:** Keyboard-only navigation

## Success Criteria
- [ ] All existing popups enhanced with canvas elements
- [ ] Mouse interaction functional in canvas areas
- [ ] Keyboard navigation maintained and enhanced
- [ ] Visual feedback improved for user actions
- [ ] Performance maintains acceptable frame rates
- [ ] Fallback rendering works when canvas unavailable
- [ ] User can configure canvas behavior preferences

## Configuration Options

### User Preferences
```rust
#[derive(Debug, Clone)]
pub struct CanvasPreferences {
    pub enable_animations: bool,
    pub preferred_marker: Marker,
    pub animation_fps: u8,
    pub enable_mouse_interaction: bool,
    pub visual_feedback_level: FeedbackLevel,
}

#[derive(Debug, Clone)]
pub enum FeedbackLevel {
    Minimal,
    Standard,
    Enhanced,
}
```

## Next Phase Dependencies
This phase enables:
- Advanced canvas features and animations
- Model visualization capabilities
- Interactive canvas games/activities
- Complex data visualization charts
