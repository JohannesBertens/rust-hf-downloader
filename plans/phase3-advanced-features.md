# Phase 3: Advanced Features

## Overview
This phase implements advanced canvas features including model visualization, enhanced progress indicators, data visualization charts, and interactive elements that leverage the full power of ratatui's canvas widget.

## Advanced Feature Implementation

### 1. Model Visualization Canvas

**File:** `src/ui/render.rs`

#### Model Architecture Visualization
```rust
fn render_model_architecture_visualization(
    ctx: &mut Context,
    model_info: &ModelInfo,
    area: Rect,
) {
    // Render model type icon
    let model_type = detect_model_type(&model_info.id);
    
    match model_type {
        ModelType::Transformer => render_transformer_architecture(ctx, area),
        ModelType::CNN => render_cnn_architecture(ctx, area),
        ModelType::GPT => render_gpt_architecture(ctx, area),
        ModelType::LSTM => render_lstm_architecture(ctx, area),
        ModelType::Unknown => render_unknown_architecture(ctx, area),
    }
    
    // Add model statistics
    render_model_statistics(ctx, model_info, area);
}

fn render_transformer_architecture(ctx: &mut Context, area: Rect) {
    let center_x = area.width as f64 / 2.0;
    let center_y = area.height as f64 / 2.0;
    
    // Input layer
    ctx.draw(&Rectangle {
        x: center_x - 40.0,
        y: center_y - 60.0,
        width: 80.0,
        height: 20.0,
        color: Color::Blue,
    });
    ctx.print(center_x - 20.0, center_y - 51.0, "Input");
    
    // Attention layers
    for i in 0..6 {
        let y = center_y - 20.0 + (i as f64 * 15.0);
        ctx.draw(&Rectangle {
            x: center_x - 30.0,
            y,
            width: 60.0,
            height: 12.0,
            color: Color::Cyan,
        });
        ctx.print(center_x - 15.0, y + 1.0, &format!("Attn {}", i));
        
        // Connection lines
        if i > 0 {
            ctx.draw(&Line::new(
                center_x,
                center_y - 20.0 + ((i-1) as f64 * 15.0) + 12.0,
                center_x,
                y,
                Color::DarkGray,
            ));
        }
    }
    
    // Output layer
    ctx.draw(&Rectangle {
        x: center_x - 40.0,
        y: center_y + 60.0,
        width: 80.0,
        height: 20.0,
        color: Color::Green,
    });
    ctx.print(center_x - 15.0, center_y + 69.0, "Output");
    
    // Final connection
    ctx.draw(&Line::new(
        center_x,
        center_y + 60.0,
        center_x,
        center_y + 60.0 - 12.0,
        Color::DarkGray,
    ));
}
```

#### Interactive Model Comparison View
```rust
fn render_model_comparison_canvas(
    ctx: &mut Context,
    models: &[ModelInfo],
    selected_models: &[usize],
    area: Rect,
) {
    let comparison_width = area.width as f64 / selected_models.len() as f64;
    
    selected_models.iter().enumerate().for_each(|(i, model_idx)| {
        let model = &models[*model_idx];
        let x_offset = i as f64 * comparison_width;
        
        // Model container
        ctx.draw(&Rectangle {
            x: x_offset + 2.0,
            y: 2.0,
            width: comparison_width - 4.0,
            height: area.height as f64 - 4.0,
            color: Color::Black,
        });
        
        // Model header
        ctx.draw(&Rectangle {
            x: x_offset + 2.0,
            y: 2.0,
            width: comparison_width - 4.0,
            height: 15.0,
            color: Color::Blue,
        });
        
        // Model name
        let model_name = &model.id.chars().take(20).collect::<String>();
        ctx.print(x_offset + 5.0, 5.0, model_name);
        
        // Statistics bars
        render_model_stats_bars(ctx, model, x_offset, area.height as f64);
    });
}

fn render_model_stats_bars(ctx: &mut Context, model: &ModelInfo, x_offset: f64, height: f64) {
    let stats_y_start = 20.0;
    let bar_width = (height - stats_y_start - 5.0) / 3.0;
    
    // Downloads bar
    let downloads_percent = (model.downloads as f64 / 100000.0).min(1.0) * 100.0;
    ctx.draw(&Rectangle {
        x: x_offset + 5.0,
        y: stats_y_start,
        width: (bar_width * downloads_percent / 100.0),
        height: 8.0,
        color: Color::Cyan,
    });
    ctx.print(x_offset + 5.0, stats_y_start - 1.0, "Downloads");
    
    // Likes bar
    let likes_percent = (model.likes as f64 / 10000.0).min(1.0) * 100.0;
    ctx.draw(&Rectangle {
        x: x_offset + 5.0,
        y: stats_y_start + 12.0,
        width: (bar_width * likes_percent / 100.0),
        height: 8.0,
        color: Color::Yellow,
    });
    ctx.print(x_offset + 5.0, stats_y_start + 11.0, "Likes");
    
    // Tags visualization
    let tag_count = model.tags.len();
    let tag_percent = (tag_count as f64 / 10.0).min(1.0) * 100.0;
    ctx.draw(&Rectangle {
        x: x_offset + 5.0,
        y: stats_y_start + 24.0,
        width: (bar_width * tag_percent / 100.0),
        height: 8.0,
        color: Color::Magenta,
    });
    ctx.print(x_offset + 5.0, stats_y_start + 23.0, "Tags");
}
```

### 2. Enhanced Progress Visualization

**File:** `src/ui/render.rs`

#### Real-time Network Activity Visualization
```rust
fn render_network_activity_canvas(
    ctx: &mut Context,
    download_progress: &DownloadProgress,
    area: Rect,
) {
    let center_x = area.width as f64 / 2.0;
    let center_y = area.height as f64 / 2.0;
    
    // Render network speed gauge
    render_speed_gauge(ctx, download_progress.speed_mbps, center_x, center_y, 25.0);
    
    // Render active connections
    let active_connections: Vec<_> = download_progress.chunks.iter()
        .filter(|chunk| chunk.is_active)
        .collect();
    
    active_connections.iter().enumerate().for_each(|(i, chunk)| {
        let angle = (i as f64 / active_connections.len() as f64) * std::f64::consts::PI * 2.0;
        let x = center_x + 30.0 * angle.cos();
        let y = center_y + 30.0 * angle.sin();
        
        let progress = (chunk.downloaded as f64 / chunk.total as f64 * 100.0) as u16;
        
        ctx.draw(&Circle {
            x,
            y,
            radius: 8.0,
            color: if progress < 50 { Color::Red } else if progress < 90 { Color::Yellow } else { Color::Green },
        });
        
        ctx.print(x - 2.0, y - 1.0, &progress.to_string());
        
        // Connection lines to center
        ctx.draw(&Line::new(
            center_x,
            center_y,
            x,
            y,
            Color::DarkGray,
        ));
    });
}

fn render_speed_gauge(ctx: &mut Context, speed_mbps: f64, center_x: f64, center_y: f64, radius: f64) {
    // Speed gauge background
    ctx.draw(&Circle {
        x: center_x,
        y: center_y,
        radius,
        color: Color::Black,
    });
    
    // Speed indicator arc
    let max_speed = 100.0; // Assume 100 Mbps max
    let speed_percent = (speed_mbps / max_speed).min(1.0);
    let end_angle = speed_percent * std::f64::consts::PI * 1.5 - std::f64::consts::PI; // 270 degrees
    
    // Draw gauge arc (simplified representation)
    for i in 0..(speed_percent * 100.0) as i32 {
        let angle = (i as f64 / 100.0) * std::f64::consts::PI * 1.5 - std::f64::consts::PI;
        let x = center_x + (radius - 2.0) * angle.cos();
        let y = center_y + (radius - 2.0) * angle.sin();
        
        ctx.draw(&Circle {
            x,
            y,
            radius: 1.5,
            color: Color::Cyan,
        });
    }
    
    // Speed text
    ctx.print(center_x - 10.0, center_y - 1.0, &format!("{:.1}", speed_mbps));
    ctx.print(center_x - 8.0, center_y + 2.0, "MB/s");
}
```

#### Verification Progress with Canvas Charts
```rust
fn render_verification_progress_chart(
    ctx: &mut Context,
    verification_progress: &[VerificationProgress],
    area: Rect,
) {
    let chart_width = area.width as f64 - 20.0;
    let chart_height = 60.0;
    let start_x = 10.0;
    let start_y = 10.0;
    
    verification_progress.iter().enumerate().for_each(|(i, ver)| {
        let y = start_y + (i as f64 * (chart_height + 10.0));
        let progress_percent = (ver.verified_bytes as f64 / ver.total_bytes as f64 * 100.0);
        
        // Progress bar background
        ctx.draw(&Rectangle {
            x: start_x,
            y,
            width: chart_width,
            height: chart_height,
            color: Color::Black,
        });
        
        // Progress bar fill
        ctx.draw(&Rectangle {
            x: start_x,
            y,
            width: chart_width * progress_percent / 100.0,
            height: chart_height,
            color: Color::Green,
        });
        
        // File name (truncated)
        let filename = &ver.filename.chars().take(30).collect::<String>();
        ctx.print(start_x, y - 1.0, filename);
        
        // Progress percentage
        ctx.print(start_x + chart_width - 15.0, y + chart_height + 1.0, 
                 &format!("{}%", progress_percent as u16));
    });
}
```

### 3. Interactive Configuration Dashboard

**File:** `src/ui/render.rs`

#### Live Configuration Preview
```rust
fn render_live_config_preview(
    ctx: &mut Context,
    current_config: &AppOptions,
    temp_config: &AppOptions,
    area: Rect,
) {
    let panel_width = area.width as f64 / 2.0;
    
    // Current config panel
    render_config_panel(ctx, current_config, 0.0, area.height, "Current", Color::DarkGray);
    
    // Temp config panel
    render_config_panel(ctx, temp_config, panel_width, area.height, "Preview", Color::Cyan);
    
    // Comparison indicators
    render_comparison_indicators(ctx, current_config, temp_config, panel_width, area.height);
}

fn render_config_panel(
    ctx: &mut Context,
    config: &AppOptions,
    x_offset: f64,
    height: f64,
    title: &str,
    color: Color,
) {
    let line_height = 3.0;
    let start_y = 5.0;
    
    // Panel header
    ctx.draw(&Rectangle {
        x: x_offset + 2.0,
        y: 2.0,
        width: height / 2.0 - 4.0,
        height: 15.0,
        color,
    });
    ctx.print(x_offset + 5.0, 5.0, title);
    
    // Configuration items
    let config_items = vec![
        ("Threads", config.concurrent_threads.to_string()),
        ("Timeout", format!("{}s", config.download_timeout_secs)),
        ("Chunks", config.num_chunks.to_string()),
        ("Retries", config.max_retries.to_string()),
        ("Verify", if config.verification_on_completion { "Yes" } else { "No" }),
    ];
    
    config_items.iter().enumerate().for_each(|(i, (label, value))| {
        let y = start_y + (i as f64 * line_height);
        
        // Label
        ctx.print(x_offset + 5.0, y, label);
        
        // Value
        ctx.print(x_offset + 40.0, y, value);
    });
}
```

### 4. Model Performance Analytics Canvas

**File:** `src/ui/render.rs`

#### Download Performance Analytics
```rust
fn render_performance_analytics_canvas(
    ctx: &mut Context,
    download_history: &[DownloadRecord],
    area: Rect,
) {
    if download_history.len() < 2 {
        return;
    }
    
    // Calculate performance metrics
    let speeds: Vec<f64> = download_history.iter()
        .map(|record| record.speed_mbps)
        .collect();
    
    let avg_speed = speeds.iter().copied().sum::<f64>() / speeds.len() as f64;
    let max_speed = speeds.iter().copied().fold(0.0, f64::max);
    let min_speed = speeds.iter().copied().fold(f64::MAX, f64::min);
    
    // Render speed chart
    render_speed_chart(ctx, &speeds, area);
    
    // Render statistics
    render_performance_stats(ctx, avg_speed, max_speed, min_speed, area);
}

fn render_speed_chart(ctx: &mut Context, speeds: &[f64], area: Rect) {
    let chart_width = area.width as f64 - 20.0;
    let chart_height = area.height as f64 - 40.0;
    let start_x = 10.0;
    let start_y = 10.0;
    
    let max_speed = speeds.iter().copied().fold(0.0, f64::max);
    
    speeds.iter().enumerate().for_each(|(i, &speed)| {
        let x = start_x + (i as f64 * chart_width / speeds.len() as f64);
        let bar_height = (speed / max_speed) * chart_height;
        let y = start_y + chart_height - bar_height;
        
        // Speed bar
        ctx.draw(&Rectangle {
            x,
            y,
            width: chart_width / speeds.len() - 2.0,
            height: bar_height,
            color: Color::Blue,
        });
    });
}
```

## Advanced Interaction Features

### File:** `src/ui/app/events.rs`

#### Canvas Gesture Recognition
```rust
async fn handle_canvas_gestures(&mut self, events: Vec<Event>) {
    for event in events {
        match event {
            Event::Mouse(mouse_event) => {
                self.handle_canvas_mouse_gestures(mouse_event).await;
            }
            Event::Key(key_event) => {
                self.handle_canvas_key_gestures(key_event);
            }
            _ => {}
        }
    }
}

fn handle_canvas_mouse_gestures(&mut self, mouse_event: MouseEvent) {
    match mouse_event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Handle click and drag gestures
            if self.is_dragging {
                self.update_drag_operation(mouse_event.column, mouse_event.row);
            } else {
                self.begin_drag_operation(mouse_event.column, mouse_event.row);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            self.end_drag_operation();
        }
        MouseEventKind::Moved => {
            self.update_hover_detection(mouse_event.column, mouse_event.row);
        }
        _ => {}
    }
}
```

## Performance Optimizations

### Canvas Rendering Pipeline
**File:** `src/ui/render.rs`

```rust
struct CanvasRenderPipeline {
    dirty_rectangles: Vec<Rect>,
    cached_elements: HashMap<String, CanvasElement>,
    frame_counter: u64,
}

impl CanvasRenderPipeline {
    fn render_frame(&mut self, ctx: &mut Context, area: Rect) {
        self.frame_counter += 1;
        
        // Clear only dirty rectangles
        for rect in &self.dirty_rectangles {
            self.clear_rectangle(ctx, *rect);
        }
        
        // Render cached elements
        for element in self.cached_elements.values() {
            element.render(ctx);
        }
        
        // Render new elements
        self.render_new_elements(ctx, area);
        
        // Clear dirty rectangles
        self.dirty_rectangles.clear();
    }
}
```

## State Management for Advanced Features

**File:** `src/ui/app/state.rs`

```rust
pub struct AdvancedCanvasState {
    pub model_visualization: ModelVisualizationState,
    pub performance_analytics: PerformanceAnalyticsState,
    pub interactive_config: InteractiveConfigState,
    pub gesture_recognition: GestureRecognitionState,
}

#[derive(Debug, Clone)]
pub struct ModelVisualizationState {
    pub selected_models: Vec<usize>,
    pub comparison_mode: bool,
    pub zoom_level: f64,
    pub pan_offset: (f64, f64),
}

#[derive(Debug, Clone)]
pub struct PerformanceAnalyticsState {
    pub history_data: Vec<DownloadRecord>,
    pub chart_type: ChartType,
    pub time_range: TimeRange,
}

#[derive(Debug, Clone)]
pub enum ChartType {
    Line,
    Bar,
    Scatter,
    Area,
}
```

## Testing Strategy

### Advanced Feature Testing
1. **Canvas Performance Tests:** Benchmark canvas rendering with complex visualizations
2. **Interactive Tests:** Test mouse and keyboard interactions with canvas elements
3. **Memory Usage Tests:** Monitor memory consumption during complex canvas operations
4. **Compatibility Tests:** Test advanced features across different terminal emulators

## Success Criteria
- [ ] Model visualization renders correctly for different model types
- [ ] Interactive configuration dashboard functions properly
- [ ] Performance analytics provide meaningful insights
- [ ] Real-time network visualization updates smoothly
- [ ] Gesture recognition responds accurately
- [ ] Advanced features maintain acceptable frame rates
- [ ] Canvas pipeline optimization reduces redraw overhead

## Next Phase Dependencies
This phase enables:
- Full interactive canvas applications
- Complex data visualization dashboards
- Advanced user interface patterns
- Real-time collaboration features
