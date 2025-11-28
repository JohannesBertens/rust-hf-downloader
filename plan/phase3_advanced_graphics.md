# Phase 3: Advanced Graphics

This phase implements the most sophisticated visual enhancements including gradients, interactive previews, and dynamic theming systems.

## Overview

**Objective**: Add advanced graphics features like gradient backgrounds, interactive filter previews, and dynamic theming to create a highly polished visual experience.

**Target Files**: 
- `src/ui/render.rs` - Main rendering updates
- `src/ui/advanced_graphics.rs` - New module for advanced graphics
- `src/ui/themes.rs` - New module for theme management

## Documentation References

- [Ratatui Canvas Advanced](https://ratatui.rs/concepts/rendering/canvas/advanced/): Advanced canvas techniques
- [Ratatui Animation](https://ratatui.rs/highlights/v027/): Animation and modifier effects
- [Ratatui Custom Widgets](https://ratatui.rs/concepts/rendering/custom-widgets/): Building custom widgets
- [Unicode Braille Patterns](https://en.wikipedia.org/wiki/Braille_PatternS): Understanding marker resolution

## Implementation Details

### Current Implementation Analysis

The application currently uses basic panel highlighting and could benefit from:
- **Gradient Backgrounds**: Smooth color transitions for visual appeal
- **Interactive Previews**: Real-time filter preview as users type
- **Dynamic Theming**: Theme cycling based on time of day or user preference
- **Advanced Animations**: Subtle entrance/exit animations for panels

### Advanced Graphics Specifications

#### 1. Gradient Background System
Create smooth color transitions for panel backgrounds:

```rust
// Advanced gradient function
fn draw_gradient_background(
    ctx: &mut Context,
    area: Rect,
    start_color: Color,
    end_color: Color,
    direction: GradientDirection,
) {
    let steps = area.height;
    for y in 0..area.height {
        let ratio = y as f64 / steps as f64;
        let mixed_color = start_color.mix(end_color, ratio);
        
        ctx.draw(&Rectangle {
            x: 0.0,
            y: y as f64,
            width: area.width as f64,
            height: 1.0,
            color: mixed_color,
        });
    }
}

// Gradient direction enum
#[derive(Debug, Clone, PartialEq)]
pub enum GradientDirection {
    TopToBottom,
    BottomToTop,
    LeftToRight,
    RightToLeft,
    Diagonal,
}
```

#### 2. Interactive Filter Preview
Real-time preview of filter effects as users modify settings:

```rust
// Interactive preview system
pub struct FilterPreview {
    pub filter_params: FilterParams,
    pub preview_canvas: Option<Canvas>,
    pub preview_results: Vec<ModelInfo>,
}

impl FilterPreview {
    pub fn update_preview(&mut self, new_params: FilterParams) -> Result<()> {
        self.filter_params = new_params;
        
        // Recalculate filtered results
        self.preview_results = self.apply_filters(self.filter_params);
        
        // Update preview canvas
        self.preview_canvas = Some(self.create_preview_canvas());
        Ok(())
    }
    
    fn create_preview_canvas(&self) -> Canvas {
        Canvas::default()
            .background_color(Color::DarkGray)
            .paint(|ctx| {
                self.draw_filter_overlay(ctx);
                self.draw_results_preview(ctx);
            })
    }
    
    fn draw_filter_overlay(&self, ctx: &mut Context) {
        // Draw semi-transparent overlay showing active filters
        let overlay_color = Color::DarkGray.mix(Color::Black, 0.5);
        ctx.draw(&Rectangle {
            x: 0.0, y: 0.0,
            width: self.area.width as f64,
            height: 3.0,  // Filter toolbar height
            color: overlay_color,
        });
    }
    
    fn draw_results_preview(&self, ctx: &mut Context) {
        // Draw miniature result list
        let preview_results = &self.preview_results[..5.min(5, self.preview_results.len())];
        for (i, result) in preview_results.iter().enumerate() {
            let y = 3.0 + (i as f64 * 1.2);
            ctx.print(1.0, y, &format!("{}: {}", i + 1, result.id));
        }
    }
}
```

#### 3. Dynamic Theme System
Theme management with smooth transitions:

```rust
// Theme management
#[derive(Debug, Clone, PartialEq)]
pub enum Theme {
    Dark,
    Light,
    Solarized,
    Nord,
    OneDark,
    Custom(Vec<Color>),
}

pub struct ThemeManager {
    current_theme: Theme,
    transition_progress: f64,  // 0.0 to 1.0
    transition_direction: TransitionDirection,
    transition_start_time: SystemTime,
    transition_duration: Duration,
}

impl ThemeManager {
    pub fn transition_to(&mut self, new_theme: Theme) {
        self.transition_direction = TransitionDirection::Forward;
        self.transition_progress = 0.0;
        self.transition_start_time = SystemTime::now();
        self.theme_history.push(self.current_theme.clone());
        self.current_theme = new_theme;
    }
    
    pub fn update_transition(&mut self) {
        if self.transition_progress < 1.0 {
            let elapsed = self.transition_start_time.elapsed().unwrap_or_default();
            self.transition_progress = (elapsed.as_secs_f64() / self.transition_duration.as_secs_f64()).min(1.0);
        }
    }
    
    pub fn get_current_theme(&self) -> Theme {
        if self.transition_progress >= 1.0 {
            return self.current_theme.clone();
        }
        
        // Interpolate between themes during transition
        let from_theme = self.theme_history.last().unwrap_or(&Theme::Dark);
        let to_theme = &self.current_theme;
        
        self.interpolate_themes(from_theme, to_theme, self.ease_in_out(self.transition_progress))
    }
    
    fn interpolate_themes(&self, from: &Theme, to: &Theme, t: f64) -> Theme {
        match (from, to) {
            (Theme::Dark, Theme::Light) => {
                // Interpolate between dark and light
                let bg = Color::DarkGray.mix(Color::White, t);
                let fg = Color::White.mix(Color::Black, t);
                Theme::Custom(vec![bg, fg])
            },
            _ => to.clone(),
        }
    }
    
    fn ease_in_out(&self, t: f64) -> f64 {
        // Smooth easing function for transitions
        if t < 0.5 {
            2.0 * t * t
        } else {
            1.0 - (2.0 * (1.0 - t) * (1.0 - t))
        }
    }
}
```

#### 4. Advanced Animation System
Panel entrance/exit animations and focus transitions:

```rust
// Animation management
#[derive(Debug, Clone, PartialEq)]
pub enum Animation {
    None,
    FadeIn(Duration),
    SlideFrom(Direction, Duration),
    Pulse(Color, Duration),
    Bounce(Duration),
}

pub struct AnimatedPanel {
    pub animation: Animation,
    pub start_time: SystemTime,
    pub area: Rect,
}

impl AnimatedPanel {
    pub fn update_animation(&mut self) -> Option<Canvas> {
        let elapsed = self.start_time.elapsed().unwrap_or_default();
        
        match &self.animation {
            Animation::FadeIn(duration) => {
                let t = (elapsed.as_secs_f64() / duration.as_secs_f64()).min(1.0);
                if t >= 1.0 {
                    return None;  // Animation complete
                }
                Some(self.create_fade_canvas(t))
            },
            Animation::SlideFrom(direction, duration) => {
                let t = (elapsed.as_secs_f64() / duration.as_secs_f64()).min(1.0);
                if t >= 1.0 {
                    return None;
                }
                Some(self.create_slide_canvas(direction, t))
            },
            _ => None,
        }
    }
    
    fn create_fade_canvas(&self, t: f64) -> Canvas {
        let alpha = self.ease_out(t);
        let bg_color = Color::DarkGray.mix(Color::Black, alpha);
        
        Canvas::default()
            .background_color(bg_color)
            .paint(|ctx| {
                ctx.draw(&Rectangle {
                    x: 0.0,
                    y: 0.0,
                    width: self.area.width as f64,
                    height: self.area.height as f64,
                    color: bg_color,
                });
            })
    }
    
    fn create_slide_canvas(&self, direction: &Direction, t: f64) -> Canvas {
        let eased_t = self.ease_out(t);
        let offset = self.calculate_slide_offset(direction, eased_t);
        
        Canvas::default()
            .background_color(Color::DarkGray)
            .paint(|ctx| {
                // Draw panel at offset position
                self.draw_panel_at_offset(ctx, offset);
            })
    }
    
    fn calculate_slide_offset(&self, direction: &Direction, t: f64) -> (f64, f64) {
        let distance = 10.0; // pixels to slide
        let offset = distance * (1.0 - t); // Start from full offset, move to 0
        
        match direction {
            Direction::Up => (0.0, offset),
            Direction::Down => (0.0, -offset),
            Direction::Left => (offset, 0.0),
            Direction::Right => (-offset, 0.0),
        }
    }
    
    fn ease_out(&self, t: f64) -> f64 {
        // Easing function for smooth animations
        1.0 - (1.0 - t).powi(3)
    }
}
```

## Implementation Steps

### Step 1: Create Advanced Graphics Module
Create `src/ui/advanced_graphics.rs`:

```rust
// src/ui/advanced_graphics.rs
use ratatui::widgets::canvas::{Canvas, Context, Rectangle, Line, Marker};
use std::time::{SystemTime, Duration};

pub mod gradients;
pub mod animations;
pub mod filter_previews;
pub mod theme_manager;

pub use gradients::{draw_gradient_background, GradientDirection};
pub use animations::{AnimatedPanel, Animation, update_animation};
pub use filter_previews::{FilterPreview, create_filter_preview};
pub use theme_manager::{ThemeManager, Theme, transition_to};
```

### Step 2: Create Theme Manager
Create `src/ui/themes.rs`:

```rust
// src/ui/themes.rs
use ratatui::style::Color;

pub struct ThemeDefinition {
    pub name: &'static str,
    pub colors: ThemeColors,
    pub backgrounds: PanelBackgrounds,
    pub animations: AnimationSettings,
}

pub struct ThemeColors {
    pub panel_background: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub accent: Color,
    pub border_focused: Color,
    pub border_unfocused: Color,
}

pub struct PanelBackgrounds {
    pub models: PanelStyle,
    pub quantization: PanelStyle,
    pub files: PanelStyle,
    pub metadata: PanelStyle,
    pub file_tree: PanelStyle,
}

pub enum PanelStyle {
    None,
    Solid(Color),
    Gradient(GradientDirection, Color, Color),
    Pattern(PatternType),
}

pub struct AnimationSettings {
    pub enabled: bool,
    pub duration: Duration,
    pub easing: EasingFunction,
}
```

### Step 3: Update Main Render Function
Integrate advanced graphics into `src/ui/render.rs`:

```rust
// Add to imports
use crate::ui::advanced_graphics::{
    FilterPreview, ThemeManager, AnimatedPanel,
    draw_gradient_background, GradientDirection
};

pub fn render_ui_advanced(frame: &mut Frame, params: RenderParams) -> Result<()> {
    let RenderParams { ... } = params;
    
    // Initialize advanced graphics systems
    let mut filter_preview = FilterPreview::new();
    let mut theme_manager = ThemeManager::new();
    let mut animation_system = AnimationSystem::new();
    
    // Update theme transition
    theme_manager.update_transition();
    
    // Update filter preview if needed
    if params.input_mode == InputMode::Normal && params.focused_pane == FocusedPane::Models {
        filter_preview.update_preview(params.filter_params)?;
    }
    
    let current_theme = theme_manager.get_current_theme();
    
    // Render with advanced graphics
    render_with_theme(frame, chunks, current_theme, &mut filter_preview, &mut animation_system)?;
    
    Ok(())
}

fn render_with_theme(
    frame: &mut Frame,
    chunks: Vec<Rect>,
    theme: Theme,
    filter_preview: &mut FilterPreview,
    animation_system: &mut AnimationSystem,
) -> Result<()> {
    let theme_def = get_theme_definition(theme);
    
    // Render gradient background for focused panel
    if let Some(canvas) = create_panel_canvas(chunks[1], &theme_def.backgrounds.models) {
        frame.render_widget(canvas, chunks[1]);
    }
    
    // Render filter preview if active
    if let Some(preview_canvas) = &filter_preview.preview_canvas {
        frame.render_widget(preview_canvas.clone(), chunks[1]);
    }
    
    // Render animated panels
    for animated_panel in animation_system.get_active_panels() {
        if let Some(canvas) = animated_panel.update_animation() {
            frame.render_widget(canvas, animated_panel.area);
        }
    }
    
    Ok(())
}
```

### Step 4: Create Interactive Filter Controls
Add real-time filter preview integration:

```rust
// In src/ui/app/events.rs
fn handle_filter_event(app: &mut AppState, event: Event) -> Result<()> {
    match event {
        Event::Key(KeyCode::Char('1')) => {
            // Preset: Recent
            app.models_state.filter_min_downloads = 0;
            app.models_state.filter_min_likes = 0;
            app.models_state.sort_field = SortField::Modified;
            app.models_state.sort_direction = SortDirection::Descending;
            
            // Trigger preview update
            if let Some(preview) = &mut app.filter_preview {
                preview.update_preview(app.get_filter_params())?;
            }
        },
        Event::Key(KeyCode::Char('2')) => {
            // Preset: Popular
            app.models_state.filter_min_downloads = 10000;
            app.models_state.filter_min_likes = 0;
            app.models_state.sort_field = SortField::Downloads;
            app.models_state.sort_direction = SortDirection::Descending;
            
            if let Some(preview) = &mut app.filter_preview {
                preview.update_preview(app.get_filter_params())?;
            }
        },
        Event::Key(KeyCode::Char('+')) => {
            // Increase focused filter value
            let focused_field = app.models_state.focused_filter_field;
            match focused_field {
                0 => app.models_state.filter_min_downloads = app.models_state.filter_min_downloads.saturating_add(1000),
                1 => app.models_state.filter_min_likes = app.models_state.filter_min_likes.saturating_add(100),
                _ => {},
            }
            
            // Update preview in real-time
            if let Some(preview) = &mut app.filter_preview {
                preview.update_preview(app.get_filter_params())?;
            }
        },
        _ => {},
    }
    
    Ok(())
}
```

## Testing Strategy

### Manual Testing
1. **Gradient Rendering**: Verify smooth color transitions across different terminals
2. **Animation Performance**: Check that animations run smoothly without lag
3. **Filter Preview Accuracy**: Ensure preview matches actual filter results
4. **Theme Transitions**: Verify smooth theme changes without flickers
5. **Interactive Responsiveness**: Test real-time preview updates

### Automated Testing
1. **Gradient Interpolation**: Test color mixing produces expected results
2. **Animation Timing**: Verify animations complete in expected duration
3. **Filter Logic**: Ensure preview uses same filtering as main application
4. **Theme Serialization**: Test theme persistence across application restarts

### Performance Testing
1. **Frame Rate**: Maintain 60fps during animations
2. **Memory Usage**: Monitor memory consumption with canvases and themes
3. **CPU Usage**: Profile rendering cost of advanced graphics
4. **Terminal Compatibility**: Test across different terminal emulators

### Visual Regression Testing
```rust
// Screenshot comparison system
fn compare_visual_output(
    expected: &Frame,
    actual: &Frame,
    threshold: f64,
) -> bool {
    let diff = calculate_image_diff(expected, actual);
    diff < threshold
}

fn calculate_image_diff(
    frame1: &Frame,
    frame2: &Frame,
) -> f64 {
    // Implement image comparison algorithm
    // Return percentage difference (0.0 to 1.0)
}
```

## Rollback Plan

Issues can be reverted by:

1. **Disabling advanced graphics** via feature flag
2. **Replacing complex canvases** with simple background colors
3. **Removing animation system** while preserving basic highlighting
4. **Cleaning up theme management** back to single theme
5. **Disabling filter preview** while maintaining original filter behavior

```rust
// Feature flag control
#[cfg(feature = "advanced_graphics")]
{
    // Advanced graphics code
}

#[cfg(not(feature = "advanced_graphics"))]
{
    // Fallback to basic graphics
}
```

## Expected Benefits

- **Enhanced Visual Appeal**: Gradients and animations create polished look
- **Improved User Experience**: Interactive previews provide immediate feedback
- **Better Accessibility**: Dynamic theming supports user preferences
- **Reduced Cognitive Load**: Visual previews reduce need for mental simulation
- **Professional Appearance**: Advanced graphics elevate interface quality

## Technical Considerations

### Performance Impact
- **Canvas Rendering Cost**: Each canvas add computational overhead
- **Animation Overhead**: Real-time updates require efficient rendering
- **Memory Cost**: Multiple canvases and themes increase memory usage
- **Terminal Compatibility**: Advanced features may not work on all terminals

### Optimization Strategies
```rust
// Canvas pooling to reuse instances
struct CanvasPool {
    available_canvases: VecDeque<Canvas>,
    active_canvases: HashMap<PanelId, Canvas>,
}

impl CanvasPool {
    fn get_canvas(&mut self, panel_id: PanelId) -> &mut Canvas {
        if let Some(canvas) = self.active_canvases.get_mut(&panel_id) {
            return canvas;
        }
        
        // Create new canvas if not in pool
        if let Some(canvas) = self.available_canvases.pop_front() {
            self.active_canvases.insert(panel_id, canvas);
            self.active_canvases.get_mut(&panel_id).unwrap()
        } else {
            // Create new canvas
            let new_canvas = create_panel_canvas(panel_id);
            self.active_canvases.insert(panel_id, new_canvas);
            self.active_canvases.get_mut(&panel_id).unwrap()
        }
    }
}
```

### Graceful Degradation
```rust
// Detect capabilities and fallback
fn get_rendering_capabilities() -> RenderingCapabilities {
    let terminal = detect_terminal_type();
    RenderingCapabilities {
        supports_background_colors: terminal.supports_24bit_colors(),
        supports_animations: terminal.supports_bright_colors(),
        supports_braille: terminal.supports_unicode_barrows(),
    }
}

fn create_appropriate_background(
    capabilities: RenderingCapabilities,
    preferred_style: PanelStyle,
) -> PanelStyle {
    match preferred_style {
        PanelStyle::Gradient(_, _, _) => {
            if capabilities.supports_background_colors {
                preferred_style
            } else {
                PanelStyle::Solid(Color::DarkGray)  // Fallback
            }
        },
        _ => preferred_style,
    }
}
```

## Configuration System

### Configurable Features
```rust
// config.rs additions
pub struct AdvancedGraphicsConfig {
    pub enabled: bool,
    pub theme: Theme,
    pub animation_speed: f64,  // 0.0 to 2.0 multiplier
    pub filter_preview_enabled: bool,
    pub max_preview_results: usize,
    pub performance_mode: PerformanceMode,
}

pub enum PerformanceMode {
    HighQuality,  // Full features, may lag
    Balanced,     // Good features, reasonable performance
    Performance,  // Basic features, optimal performance
}

impl AdvancedGraphicsConfig {
    pub fn from_app_options(options: &AppOptions) -> Self {
        AdvancedGraphicsConfig {
            enabled: options.advanced_graphics_enabled,
            theme: options.theme,
            animation_speed: options.animation_speed,
            filter_preview_enabled: options.filter_preview_enabled,
            max_preview_results: options.max_preview_results,
            performance_mode: options.performance_mode,
        }
    }
}
```

## Next Steps

This completes the three-phase visual enhancement plan. The implementation provides:

- **Phase 1**: Immediate improvements with minimal risk
- **Phase 2**: Enhanced panel feedback with backgrounds  
- **Phase 3**: Advanced graphics with gradients and interactivity

All phases build upon each other and can be implemented incrementally based on user needs and terminal capabilities.
