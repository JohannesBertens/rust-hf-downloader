# Phase 2: Basic Filters - Sorting Controls

## Goal
Add a filter toolbar to the top bar with sorting capabilities (sort field and direction).

## Changes Required

### 1. Add Sort Enums

**File**: `src/models.rs`

Add new enums after the existing types:

```rust
/// Sort field options for model search
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortField {
    Downloads,
    Likes,
    Modified,
    Name,
}

impl Default for SortField {
    fn default() -> Self {
        SortField::Downloads
    }
}

/// Sort direction (ascending or descending)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl Default for SortDirection {
    fn default() -> Self {
        SortDirection::Descending
    }
}
```

### 2. Add Sort State to App

**File**: `src/ui/app/state.rs`

Add fields to `App` struct:

```rust
pub struct App {
    // ... existing fields ...
    
    // Filter & Sort state
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
}
```

**In `App::new()` method**, initialize new fields:

```rust
Self {
    // ... existing fields ...
    sort_field: SortField::default(),
    sort_direction: SortDirection::default(),
}
```

### 3. Add Sort Keyboard Controls

**File**: `src/ui/app/events.rs`

**In `handle_normal_mode_input()` method**, add new key handlers:

```rust
(_, KeyCode::Char('s')) => {
    // Cycle sort field: Downloads → Likes → Modified → Name → Downloads
    self.sort_field = match self.sort_field {
        SortField::Downloads => SortField::Likes,
        SortField::Likes => SortField::Modified,
        SortField::Modified => SortField::Name,
        SortField::Name => SortField::Downloads,
    };
    
    // Re-fetch with new sort
    self.clear_search_results();
    self.needs_search_models = true;
    
    self.status = format!("Sort by: {:?}", self.sort_field);
}
(KeyModifiers::SHIFT, KeyCode::Char('S')) => {
    // Toggle sort direction
    self.sort_direction = match self.sort_direction {
        SortDirection::Ascending => SortDirection::Descending,
        SortDirection::Descending => SortDirection::Ascending,
    };
    
    // Re-fetch with new direction
    self.clear_search_results();
    self.needs_search_models = true;
    
    let arrow = match self.sort_direction {
        SortDirection::Ascending => "▲",
        SortDirection::Descending => "▼",
    };
    self.status = format!("Sort direction: {:?} {}", self.sort_direction, arrow);
}
```

### 4. Create Filtered Search API

**File**: `src/api.rs`

Add new function:

```rust
/// Fetch models with sorting parameters
pub async fn fetch_models_filtered(
    query: &str,
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
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
    
    let url = format!(
        "https://huggingface.co/api/models?search={}&limit=50&sort={}&direction={}",
        urlencoding::encode(query),
        sort,
        direction
    );
    
    let response = crate::http_client::get_with_optional_token(&url, token).await?;
    let models: Vec<ModelInfo> = response.json().await?;
    
    Ok(models)
}
```

### 5. Update Search to Use Filtered API

**File**: `src/ui/app/models.rs`

**In `search_models()` method**, replace the API call:

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
    let sort_field = self.sort_field;  // NEW
    let sort_direction = self.sort_direction;  // NEW
    
    // NEW: Use fetch_models_filtered instead of fetch_models
    match crate::api::fetch_models_filtered(&query, sort_field, sort_direction, token).await {
        Ok(results) => {
            let has_results = !results.is_empty();
            let mut models_lock = models.lock().await;
            *models_lock = results;
            self.loading = false;
            self.list_state.select(Some(0));
            self.status = format!("Found {} models", models_lock.len());
            drop(models_lock);
            
            // Trigger load for first result if we have results
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

### 6. Add Filter Toolbar Rendering

**File**: `src/ui/render.rs`

**Replace the placeholder block** in `render_ui()` with:

```rust
// NEW: Render filter toolbar instead of placeholder
render_filter_toolbar(
    frame,
    chunks[0],
    sort_field,
    sort_direction,
);
```

**Add to RenderParams struct**:

```rust
pub struct RenderParams<'a> {
    // ... existing fields ...
    pub sort_field: crate::models::SortField,
    pub sort_direction: crate::models::SortDirection,
}
```

**Add new function** at the end of the file:

```rust
/// Render filter and sort toolbar
pub fn render_filter_toolbar(
    frame: &mut Frame,
    area: Rect,
    sort_field: crate::models::SortField,
    sort_direction: crate::models::SortDirection,
) {
    use crate::models::{SortField, SortDirection};
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Filters & Sort  [/: Search | s: Cycle Sort | S: Toggle Direction]")
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
        SortField::Modified => "Last Modified",
        SortField::Name => "Name",
    };
    
    // Build display line
    let line_text = format!("Sort: {} {}", sort_name, sort_arrow);
    
    let line = Line::from(vec![
        Span::styled("Sort: ", Style::default().fg(Color::Yellow)),
        Span::styled(
            format!("{} {}", sort_name, sort_arrow),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
    ]);
    
    let paragraph = Paragraph::new(line)
        .style(Style::default());
    
    frame.render_widget(paragraph, inner);
}
```

### 7. Update Main Render Call

**File**: `src/main.rs` (or render loop location)

Update the `render_ui()` call to pass new parameters:

```rust
ui::render_ui(&mut frame, ui::render::RenderParams {
    // ... existing params ...
    sort_field: app.sort_field,
    sort_direction: app.sort_direction,
});
```

## Testing Checklist

- [ ] Press `s` cycles through sort fields (Downloads → Likes → Modified → Name)
- [ ] Press `S` (Shift+s) toggles sort direction (▲ ↔ ▼)
- [ ] Filter toolbar displays current sort field and direction
- [ ] API calls include correct sort parameters
- [ ] Results are sorted correctly according to selection
- [ ] Status bar shows sort changes
- [ ] Sort persists when switching between models
- [ ] Search popup still works correctly

## Visual Reference

```
┌─ Filters & Sort  [/: Search | s: Cycle Sort | S: Toggle Direction] ───┐
│ Sort: Downloads ▼                                                      │
└────────────────────────────────────────────────────────────────────────┘
┌─ Results ──────────────────────────────────────────────────────────────┐
│ >> 1. TheBloke/Llama-2-7B-GGUF               ↓50000000 ♥25000         │
│    2. meta-llama/Llama-3.1-8B                ↓45000000 ♥30000         │
│    3. mistralai/Mistral-7B-v0.1              ↓40000000 ♥22000         │
```

After pressing `s`:
```
┌─ Filters & Sort  [/: Search | s: Cycle Sort | S: Toggle Direction] ───┐
│ Sort: Likes ▼                                                          │
└────────────────────────────────────────────────────────────────────────┘
```

After pressing `S`:
```
┌─ Filters & Sort  [/: Search | s: Cycle Sort | S: Toggle Direction] ───┐
│ Sort: Likes ▲                                                          │
└────────────────────────────────────────────────────────────────────────┘
```

## Notes

- Sort state changes immediately trigger a new search
- Default sort is Downloads ▼ (most popular first)
- Sort state is NOT persisted in this phase (coming in Phase 4)
- The toolbar title provides quick keyboard reference
- Sort field names match HuggingFace API parameters
