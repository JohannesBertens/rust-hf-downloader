# Phase 2: Panel Background Highlighting

This phase adds subtle background colors and patterns to focused panels using ratatui's Canvas widget capabilities.

## Overview

**Objective**: Enhance panel focus indication by adding background colors and subtle patterns to panels when they have keyboard focus.

**Target Files**: 
- `src/ui/render.rs` - Panel rendering functions
- Potentially new module: `src/ui/canvas_backgrounds.rs` for reusable Canvas patterns

## Documentation References

- [Ratatui Canvas](https://docs.rs/ratatui/latest/ratatui/widgets/canvas/struct.Canvas.html): Canvas widget API
- [Canvas Shapes](https://ratatui.rs/concepts/rendering/canvas/shapes/): Rectangle, Line, and other shape drawing
- [Canvas Markers](https://ratatui.rs/concepts/rendering/canvas/markers/): Braille, HalfBlock, and custom markers
- [Canvas Colors](https://ratatui.rs/concepts/rendering/canvas/colors/): Per-cell foreground and background colors

## Implementation Details

### Current Implementation Analysis

The current panel highlighting system uses border-based indication:

```rust
// From src/ui/render.rs around lines 158, 363, 444, 561, 624
.border_style(
    if input_mode == InputMode::Normal && focused_pane == FocusedPane::Models {
        Style::default().fg(Color::Yellow)  // Current highlighting
    } else {
        Style::default()
    }
)
```

This provides good visual feedback but could be enhanced with background colors.

### Enhanced Panel Highlighting Approaches

#### 1. Canvas-Based Background Highlighting
Add a subtle background color to focused panels:

```rust
// Enhanced border style with background
.border_style(
    if input_mode == InputMode::Normal && focused_pane == FocusedPane::Models {
        Style::default()
            .fg(Color::Yellow)        // Yellow border
            .bg(Color::DarkGray)      // Dark gray background
    } else {
        Style::default()
    }
)
```

#### 2. Canvas Pattern Background
For more advanced highlighting, use a Canvas widget to paint panel backgrounds:

```rust
// Create a canvas with a subtle pattern for focused panels
use ratatui::widgets::canvas::{Canvas, Rectangle};

let panel_canvas = if input_mode == InputMode::Normal && focused_pane == FocusedPane::Models {
    Some(Canvas::default()
        .background_color(Color::DarkGray)  // Semi dark background
        .marker(Marker::HalfBlock)           // Higher resolution grid
        .paint(|ctx| {
            // Draw a subtle diagonal pattern
            for i in 0..area.height {
                if i % 2 == 0 {
                    ctx.draw(&Rectangle {
                        x: 0.0,
                        y: i as f64,
                        width: area.width as f64,
                        height: 1.0,
                        color: Color::DarkGray,
                    });
                }
            }
        }))
} else {
    None
};
```

#### 3. Focus State Background
Add background colors that change based on focus state:

```rust
// Determine background color based on focus state
let background_color = match (input_mode, focused_pane) {
    (InputMode::Normal, FocusedPane::Models) => Color::DarkGray,
    (InputMode::Normal, FocusedPane::QuantizationGroups) => Color::DarkGray,
    (InputMode::Normal, FocusedPane::QuantizationFiles) => Color::DarkGray,
    (InputMode::Normal, FocusedPane::ModelMetadata) => Color::DarkGray,
    (InputMode::Normal, FocusedPane::FileTree) => Color::DarkGray,
    _ => Color::Reset,  // No background for non-focused or non-normal mode
};

let border_style = if background_color != Color::Reset {
    Style::default()
        .fg(Color::Yellow)
        .bg(background_color)
} else {
    Style::default()
};
```

### Canvas Widget Integration

#### Canvas Setup for Panel Backgrounds
```rust
// In render_ui() function, create a canvas for each panel that needs background highlighting
let focused_panel_canvas = match focused_pane {
    FocusedPane::Models => {
        if input_mode == InputMode::Normal {
            Some(Canvas::default()
                .background_color(Color::DarkGray)
                .marker(Marker::HalfBlock)
                .x_bounds([0.0, chunks[1].width as f64])
                .y_bounds([0.0, chunks[1].height as f64])
                .paint(|ctx| {
                    // Draw subtle background pattern
                    ctx.draw(&Rectangle {
                        x: 0.0,
                        y: 0.0,
                        width: chunks[1].width as f64,
                        height: chunks[1].height as f64,
                        color: Color::DarkGray,
                    });
                }))
        } else {
            None
        }
    },
    _ => None,
};
```

#### Pattern Drawing Functions
```rust
// Create reusable pattern functions
fn draw_subtle_background(ctx: &mut Context, area: Rect, color: Color) {
    // Draw a subtle checker pattern
    for y in (0..area.height).step_by(2) {
        for x in (0..area.width).step_by(4) {
            ctx.print(x as f64, y as f64, "█".bg(color));
        }
    }
}

fn draw_gradient_background(ctx: &mut Context, area: Rect) {
    // Draw a subtle gradient effect
    for y in 0..area.height {
        let alpha = y as f64 / area.height as f64;
        let color = Color::DarkGray.mix(Color::Black, alpha);
        ctx.draw(&Rectangle {
            x: 0.0,
            y: y as f64,
            width: area.width as f64,
            height: 1.0,
            color,
        });
    }
}
```

## Implementation Steps

### Step 1: Create Canvas Background Module
Create `src/ui/canvas_backgrounds.rs`:

```rust
// src/ui/canvas_backgrounds.rs
use ratatui::widgets::canvas::{Canvas, Context, Rectangle, Marker};

pub enum PanelBackgroundStyle {
    None,
    Solid(Color),
    CheckedPattern(Color),
    Gradient,
}

pub fn create_panel_canvas(
    area: Rect,
    style: PanelBackgroundStyle,
) -> Option<Canvas> {
    match style {
        PanelBackgroundStyle::Solid(color) => Some(
            Canvas::default()
                .background_color(color)
                .marker(Marker::HalfBlock)
                .x_bounds([0.0, area.width as f64])
                .y_bounds([0.0, area.height as f64])
                .paint(move |ctx| {
                    ctx.draw(&Rectangle {
                        x: 0.0,
                        y: 0.0,
                        width: area.width as f64,
                        height: area.height as f64,
                        color,
                    });
                })
        ),
        _ => None,
    }
}
```

### Step 2: Update Panel Rendering
In `src/ui/render.rs`, update each panel rendering section:

**OLD** (around line 158):
```rust
let list = List::new(items)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(list_title)
            .border_style(
                if input_mode == InputMode::Normal && focused_pane == FocusedPane::Models {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                }
            ),
    )
```

**NEW**:
```rust
// Determine background style based on focus state
let (border_style, background_style) = match (input_mode, focused_pane) {
    (InputMode::Normal, FocusedPane::Models) => (
        Style::default().fg(Color::Yellow).bg(Color::DarkGray),
        PanelBackgroundStyle::Solid(Color::DarkGray)
    ),
    _ => (Style::default(), PanelBackgroundStyle::None),
};

let list = List::new(items)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(list_title)
            .border_style(border_style)
    );

// Render background canvas if needed
if let Some(canvas) = create_panel_canvas(chunks[1], background_style) {
    frame.render_widget(canvas, chunks[1]);
}
```

### Step 3: Background Rendering Integration
Integrate canvas backgrounds into the main render function:

```rust
// In render_ui() after setting up chunks
// Create background canvases for focused panels
let focused_panel_canvases = match focused_pane {
    FocusedPane::Models => {
        if input_mode == InputMode::Normal {
            Some(create_panel_canvas(chunks[1], PanelBackgroundStyle::Solid(Color::DarkGray)))
        } else {
            None
        }
    },
    FocusedPane::QuantizationGroups => {
        if input_mode == InputMode::Normal {
            Some(create_panel_canvas(chunks[2], PanelBackgroundStyle::Solid(Color::DarkGray)))
        } else {
            None
        }
    },
    _ => None,
};

// Render backgrounds behind panels
for canvas in focused_panel_canvases.flatten() {
    frame.render_widget(canvas, area);
}
```

### Step 4: Performance Optimization
Optimize canvas rendering to minimize redraws:

```rust
// Only recreate canvases when focus changes
use std::cell::RefCell;
use std::rc::Rc;

struct RenderState {
    last_focused_pane: Option<FocusedPane>,
    canvases: HashMap<FocusedPane, Canvas>,
}

impl RenderState {
    fn get_or_create_canvas(&mut self, focused_pane: FocusedPane, area: Rect) -> &Canvas {
        if self.last_focused_pane != Some(focused_pane) {
            self.last_focused_pane = Some(focused_pane);
            self.canvses.insert(focused_pane, create_panel_canvas(area, PanelBackgroundStyle::Solid(Color::DarkGray)));
        }
        self.canvses.get(&focused_pane).unwrap()
    }
}
```

## Testing Strategy

### Manual Testing
1. **Focus Navigation**: Check that background appears/disappears correctly when navigating between panels
2. **Performance**: Verify that background rendering doesn't cause lag during rapid navigation
3. **Terminal Compatibility**: Ensure backgrounds work across different terminal emulators
4. **Accessibility**: Verify sufficient color contrast for readability

### Visual Verification Tests
1. **Before/After Comparison**: Take screenshots with and without backgrounds
2. **Focus State Testing**: Test all 5 panels (Models, QuantizationGroups, QuantizationFiles, ModelMetadata, FileTree)
3. **Color Calibration**: Verify colors look correct on dark and light terminal themes

### Performance Testing
1. **Navigation Speed**: Time focus changes with backgrounds enabled
2. **Memory Usage**: Monitor memory consumption with canvas instances
3. **Redraw Optimization**: Verify minimal redraws when focus doesn't change

## Rollback Plan

Issues can be reverted by:

1. **Removing canvas imports and usage** from `src/ui/render.rs`
2. **Reverting border style changes** back to original yellow-only highlighting
3. **Deleting `src/ui/canvas_backgrounds.rs`** if created
4. **Cleaning up unused imports** from `use ratatui::widgets::canvas`

## Expected Benefits

- **Enhanced Focus Feedback**: Background colors make focused panels more apparent
- **Improved Accessibility**: Better visual separation for users with reduced color perception
- **Reduced Cognitive Load**: Clearer indication of active area reduces mental effort to track focus
- **Better Visual Hierarchy**: Backgrounds create stronger distinction between active/inactive areas

## Technical Considerations

### Terminal Compatibility
- **Background Color Support**: Not all terminals support per-cell background colors
- **Marker Support**: Braille and HalfBlock markers may not render correctly in all fonts
- **Performance Impact**: Canvas rendering adds computational overhead

### Fallback Strategy
```rust
// Detect terminal capabilities and fallback gracefully
let supports_background_colors = detect_terminal_capabilities().background_colors;
let supports_braille = detect_terminal_capabilities().braille_markers;

let marker = if supports_braille {
    Marker::HalfBlock
} else {
    Marker::Braille  // Fallback to standard grid
};
```

### Performance Monitoring
```rust
use std::time::Instant;

let render_start = Instant::now();
// Canvas rendering code
let render_duration = render_start.elapsed();

if render_duration > std::time::Duration::from_millis(16) {
    log::warn!("Canvas rendering took {}ms, consider optimization", render_duration.as_millis());
}
```

## Next Steps

Once this phase is complete, proceed to **Phase 3** for advanced graphics including gradients and interactive previews.
