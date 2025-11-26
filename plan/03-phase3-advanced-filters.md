# Phase 3: Advanced Filters - Numeric Filters & Navigation

## Goal
Add numeric filters (min downloads, min likes) with keyboard navigation to create a power-user filtering system.

## Changes Required

### 1. Add Filter State to App

**File**: `src/ui/app/state.rs`

Add fields to `App` struct:

```rust
pub struct App {
    // ... existing fields ...
    
    // Filter & Sort state
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    pub filter_min_downloads: u64,     // NEW
    pub filter_min_likes: u64,         // NEW
    pub focused_filter_field: usize,   // NEW: 0=sort, 1=downloads, 2=likes
}
```

**In `App::new()` method**, initialize new fields:

```rust
Self {
    // ... existing fields ...
    sort_field: SortField::default(),
    sort_direction: SortDirection::default(),
    filter_min_downloads: 0,           // NEW
    filter_min_likes: 0,                // NEW
    focused_filter_field: 0,            // NEW
}
```

### 2. Add Filter Navigation Controls

**File**: `src/ui/app/events.rs`

**In `handle_normal_mode_input()` method**, add new key handlers:

```rust
(_, KeyCode::Char('f')) => {
    // Cycle focused filter field: sort → min_downloads → min_likes → sort
    self.focused_filter_field = (self.focused_filter_field + 1) % 3;
    
    let field_name = match self.focused_filter_field {
        0 => "Sort",
        1 => "Min Downloads",
        2 => "Min Likes",
        _ => unreachable!(),
    };
    self.status = format!("Focused filter: {}", field_name);
}
(_, KeyCode::Char('+') | KeyCode::Right) if self.input_mode == InputMode::Normal => {
    // Increment focused filter
    self.modify_focused_filter(1);
}
(_, KeyCode::Char('-') | KeyCode::Left) if self.input_mode == InputMode::Normal => {
    // Decrement focused filter
    self.modify_focused_filter(-1);
}
(_, KeyCode::Char('r')) => {
    // Reset all filters to defaults
    self.sort_field = SortField::default();
    self.sort_direction = SortDirection::default();
    self.filter_min_downloads = 0;
    self.filter_min_likes = 0;
    self.focused_filter_field = 0;
    
    // Re-fetch with reset filters
    self.clear_search_results();
    self.needs_search_models = true;
    
    self.status = "Filters reset to defaults".to_string();
}
```

**Add new helper method** to `impl App`:

```rust
/// Modify the focused filter field by delta
fn modify_focused_filter(&mut self, delta: i64) {
    match self.focused_filter_field {
        0 => {
            // Cycle sort field with + or toggle direction with -
            if delta > 0 {
                self.sort_field = match self.sort_field {
                    SortField::Downloads => SortField::Likes,
                    SortField::Likes => SortField::Modified,
                    SortField::Modified => SortField::Name,
                    SortField::Name => SortField::Downloads,
                };
            } else {
                self.sort_direction = match self.sort_direction {
                    SortDirection::Ascending => SortDirection::Descending,
                    SortDirection::Descending => SortDirection::Ascending,
                };
            }
        }
        1 => {
            // Min downloads: 0, 100, 1000, 10000, 100000, 1000000
            let steps = [0, 100, 1000, 10000, 100000, 1000000];
            let current_idx = steps.iter().position(|&s| s == self.filter_min_downloads)
                .unwrap_or(0);
            
            let new_idx = if delta > 0 {
                (current_idx + 1).min(steps.len() - 1)
            } else {
                current_idx.saturating_sub(1)
            };
            
            self.filter_min_downloads = steps[new_idx];
        }
        2 => {
            // Min likes: 0, 10, 50, 100, 500, 1000, 5000
            let steps = [0, 10, 50, 100, 500, 1000, 5000];
            let current_idx = steps.iter().position(|&s| s == self.filter_min_likes)
                .unwrap_or(0);
            
            let new_idx = if delta > 0 {
                (current_idx + 1).min(steps.len() - 1)
            } else {
                current_idx.saturating_sub(1)
            };
            
            self.filter_min_likes = steps[new_idx];
        }
        _ => return,
    }
    
    // Re-fetch with new filters
    self.clear_search_results();
    self.needs_search_models = true;
}
```

### 3. Update Filtered API with Client-Side Filtering

**File**: `src/api.rs`

Update `fetch_models_filtered()` to accept filter parameters:

```rust
/// Fetch models with sorting and filtering parameters
pub async fn fetch_models_filtered(
    query: &str,
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
    min_downloads: u64,  // NEW
    min_likes: u64,      // NEW
    token: Option<&String>,
) -> Result<Vec<ModelInfo>, reqwest::Error> {
    use crate::models::{SortField, SortDirection};
    
    let sort = match sort_field {
        SortField::Downloads => "downloads",
        SortField::Likes => "likes",
        SortField::Modified => "lastModified",
        SortField::Name => "id",
    };
    
    let direction = match sort_direction {
        SortDirection::Ascending => "1",
        SortDirection::Descending => "-1",
    };
    
    // Request more results (100) since we'll filter client-side
    let url = format!(
        "https://huggingface.co/api/models?search={}&limit=100&sort={}&direction={}",
        urlencoding::encode(query),
        sort,
        direction
    );
    
    let response = crate::http_client::get_with_optional_token(&url, token).await?;
    let mut models: Vec<ModelInfo> = response.json().await?;
    
    // NEW: Client-side filtering (API doesn't support these filters)
    models.retain(|m| {
        m.downloads >= min_downloads && m.likes >= min_likes
    });
    
    Ok(models)
}
```

### 4. Update Search to Pass Filters

**File**: `src/ui/app/models.rs`

**In `search_models()` method**, pass filter parameters:

```rust
pub async fn search_models(&mut self) {
    let query = self.input.value().to_string();
    
    if query.is_empty() {
        return;
    }

    self.loading = true;
    self.error = None;
    
    let models = self.models.clone();
    let token = self.options.hf_token.as_ref();
    let sort_field = self.sort_field;
    let sort_direction = self.sort_direction;
    let min_downloads = self.filter_min_downloads;  // NEW
    let min_likes = self.filter_min_likes;          // NEW
    
    match crate::api::fetch_models_filtered(
        &query,
        sort_field,
        sort_direction,
        min_downloads,  // NEW
        min_likes,      // NEW
        token
    ).await {
        Ok(results) => {
            let has_results = !results.is_empty();
            let mut models_lock = models.lock().await;
            *models_lock = results;
            self.loading = false;
            self.list_state.select(Some(0));
            
            // NEW: Show filter count in status
            let filter_status = if min_downloads > 0 || min_likes > 0 {
                format!(" (filtered from 100)")
            } else {
                String::new()
            };
            self.status = format!("Found {} models{}", models_lock.len(), filter_status);
            
            drop(models_lock);
            
            if has_results {
                self.needs_load_quantizations = true;
            }
        }
        Err(e) => {
            self.loading = false;
            self.error = Some(format!("Failed to fetch models: {}", e));
            self.status = "Search failed".to_string();
        }
    }
}
```

### 5. Update Filter Toolbar Rendering

**File**: `src/ui/render.rs`

**Update `render_filter_toolbar()` signature and implementation**:

```rust
/// Render filter and sort toolbar
pub fn render_filter_toolbar(
    frame: &mut Frame,
    area: Rect,
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
    min_downloads: u64,        // NEW
    min_likes: u64,            // NEW
    focused_field: usize,      // NEW
) {
    use crate::models::{SortField, SortDirection};
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Filters & Sort  [/: Search | f: Focus | +/-: Modify | r: Reset]")
        .style(Style::default().fg(Color::Cyan));
    
    let inner = block.inner(area);
    frame.render_widget(block, area);
    
    // Sort arrow
    let sort_arrow = match sort_direction {
        SortDirection::Ascending => "▲",
        SortDirection::Descending => "▼",
    };
    
    // Sort name
    let sort_name = match sort_field {
        SortField::Downloads => "Downloads",
        SortField::Likes => "Likes",
        SortField::Modified => "Modified",
        SortField::Name => "Name",
    };
    
    // Build display line with highlighting for focused field
    let sort_style = if focused_field == 0 {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    };
    
    let downloads_style = if focused_field == 1 {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White)
    };
    
    let likes_style = if focused_field == 2 {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::White)
    };
    
    let line = Line::from(vec![
        Span::styled("Sort: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{} {}", sort_name, sort_arrow), sort_style),
        Span::raw("  |  "),
        Span::styled("Min Downloads: ", Style::default().fg(Color::DarkGray)),
        Span::styled(crate::utils::format_number(min_downloads), downloads_style),
        Span::raw("  |  "),
        Span::styled("Min Likes: ", Style::default().fg(Color::DarkGray)),
        Span::styled(crate::utils::format_number(min_likes), likes_style),
    ]);
    
    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, inner);
}
```

**Update RenderParams struct**:

```rust
pub struct RenderParams<'a> {
    // ... existing fields ...
    pub sort_field: crate::models::SortField,
    pub sort_direction: crate::models::SortDirection,
    pub filter_min_downloads: u64,    // NEW
    pub filter_min_likes: u64,        // NEW
    pub focused_filter_field: usize,  // NEW
}
```

**Update render call** in main loop:

```rust
render_filter_toolbar(
    frame,
    chunks[0],
    sort_field,
    sort_direction,
    filter_min_downloads,     // NEW
    filter_min_likes,         // NEW
    focused_filter_field,     // NEW
);
```

### 6. Update Main Render Call

**File**: `src/main.rs`

Pass new parameters to render:

```rust
ui::render_ui(&mut frame, ui::render::RenderParams {
    // ... existing params ...
    sort_field: app.sort_field,
    sort_direction: app.sort_direction,
    filter_min_downloads: app.filter_min_downloads,    // NEW
    filter_min_likes: app.filter_min_likes,            // NEW
    focused_filter_field: app.focused_filter_field,    // NEW
});
```

## Testing Checklist

- [ ] Press `f` cycles focus between sort/downloads/likes
- [ ] Focused field is highlighted (yellow + underlined)
- [ ] Press `+` increments focused filter value
- [ ] Press `-` decrements focused filter value
- [ ] Min downloads steps: 0 → 100 → 1k → 10k → 100k → 1M
- [ ] Min likes steps: 0 → 10 → 50 → 100 → 500 → 1k → 5k
- [ ] Results are filtered correctly (only shows models meeting criteria)
- [ ] Press `r` resets all filters to defaults
- [ ] Status bar shows filter count when active
- [ ] API fetches 100 results for better filtering

## Visual Reference

```
┌─ Filters & Sort  [/: Search | f: Focus | +/-: Modify | r: Reset] ────┐
│ Sort: Downloads ▼  |  Min Downloads: 10k  |  Min Likes: 100          │
└────────────────────────────────────────────────────────────────────────┘
```

With min_downloads focused:
```
┌─ Filters & Sort  [/: Search | f: Focus | +/-: Modify | r: Reset] ────┐
│ Sort: Downloads ▼  |  Min Downloads: 10k  |  Min Likes: 100          │
│                         ═══════════════                               │
└────────────────────────────────────────────────────────────────────────┘
```

## Notes

- Filters apply client-side since HF API doesn't support all parameters
- Requesting 100 results provides better filtering coverage
- Focus indicator (underline) shows which filter will be modified by +/-
- Filter values use preset steps for consistent UX
- Reset key `r` provides quick way to clear all filters
- Status bar indicates when filters are active
