# Phase 1: Add Tag Filtering - Alternative 4: Search-Based Tag Filtering

## Overview
Integrate tag filtering into the existing search popup system, allowing users to type tag names to quickly filter and select tags using text-based input.

## Implementation Strategy

### State Modifications
Add to `App` struct in `src/ui/app/state.rs`:
```rust
pub search_mode: SearchMode,                // Extend existing input handling
pub available_tags: Vec<String>,           // Cached unique tags
pub tag_search_results: Vec<String>,       // Filtered tag results
pub selected_tags: HashSet<String>,        // Active tag selections
```

Add to `models.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    ModelSearch,        // Existing: search for model names
    TagSearch,         // NEW: search for tags
}
```

### Search Mode Integration
Extend the existing search popup in `src/ui/app/events.rs`:

**Enhanced Search Popup:**
The existing search popup already exists, so we extend it to support both model and tag search:

```rust
// In handle_search_popup_input()
async fn handle_search_popup_input(&mut self, key: KeyEvent) {
    match key.code {
        KeyCode::Tab => {
            // Toggle between model search and tag search
            self.search_mode = match self.search_mode {
                SearchMode::ModelSearch => SearchMode::TagSearch,
                SearchMode::TagSearch => SearchMode::ModelSearch,
            };
            
            // Update status based on mode
            match self.search_mode {
                SearchMode::ModelSearch => self.status = "Model Search".to_string(),
                SearchMode::TagSearch => self.status = "Tag Search".to_string(),
            }
        }
        // ... existing handling for other keys ...
    }
}
```

**Search Mode Key Binding:**
Add to main event handler:
```rust
// In handle_normal_mode_input
(_, KeyCode::Char('t')) => {
    // Enter tag search mode (or toggle if already in search)
    if self.popup_mode == PopupMode::SearchPopup {
        self.search_mode = SearchMode::TagSearch;
        self.status = "Tag Search - Type to filter tags".to_string();
    } else {
        // Start new search in tag mode
        self.popup_mode = PopupMode::SearchPopup;
        self.search_mode = SearchMode::TagSearch;
        self.input.reset();
        self.status = "Tag Search".to_string();
    }
}
```

### Tag Search Logic
Implement tag filtering logic in `src/ui/app/events.rs`:

```rust
fn update_tag_search_results(&mut self) {
    let query = self.input.value().to_lowercase();
    
    if query.is_empty() {
        self.tag_search_results = self.available_tags.clone();
    } else {
        self.tag_search_results = self.available_tags
            .iter()
            .filter(|tag| tag.to_lowercase().contains(&query))
            .collect();
    }
    
    // Limit results for performance
    if self.tag_search_results.len() > 20 {
        self.tag_search_results.truncate(20);
    }
}

/// Apply tag search to filtering
fn apply_tag_filter(&mut self, tags: &[String]) {
    self.selected_tags = tags.iter().collect::<HashSet<_>>()
        .intersection(&self.selected_tags.into())
        .collect(); // Keep intersection of existing and new filters
}
```

### Enhanced Search Popup UI
Update search popup rendering in `src/ui/render.rs`:

**Dual-Mode Search Display:**
```rust
pub fn render_search_popup(
    frame: &mut Frame,
    area: Rect,
    input: &Input,
    search_mode: SearchMode,
    tag_results: &[String],
    selected_tags: &HashSet<String>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(match search_mode {
            SearchMode::ModelSearch => "Search Models  [Tab: Tag Search]",
            SearchMode::TagSearch => "Search Tags  [Tab: Model Search | Enter: Select]",
        })
        .style(Style::default().fg(Color::Green));
    
    let inner = block.inner(area);
    frame.render_widget(block, area);
    
    match search_mode {
        SearchMode::ModelSearch => {
            render_model_search(frame, inner, input);
        }
        SearchMode::TagSearch => {
            render_tag_search(frame, inner, input, tag_results, selected_tags);
        }
    }
}

fn render_tag_search(
    frame: &mut Frame,
    area: Rect,
    input: &Input,
    tag_results: &[String],
    selected_tags: &HashSet<String>,
) {
    // Input field
    let input_block = Paragraph::new(format!("Tags: {}", input.value()));
    frame.render_widget(input_block, area);
    
    // Results list (below input)
    let results_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height - 1,
    };
    
    if !tag_results.is_empty() {
        let items: Vec<ListItem> = tag_results
            .iter()
            .map(|tag| {
                let style = if selected_tags.contains(tag) {
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Span::styled(tag, style))
            })
            .collect();
        
        let list = List::new(items)
            .block(Block::default().borders(Borders::LEFT));
        
        frame.render_widget(list, results_area);
    } else {
        let no_results = Paragraph::new("No matching tags found");
        frame.render_widget(no_results, results_area);
    }
}
```

### Tag Selection Integration
Extend the search popup to handle tag selection:

```rust
// In handle_search_popup_input for TagSearch mode
KeyCode::Enter => {
    if self.search_mode == SearchMode::TagSearch {
        if let Some(selected_tag) = self.get_current_tag_selection() {
            if self.selected_tags.contains(&selected_tag) {
                self.selected_tags.remove(&selected_tag);
            } else {
                self.selected_tags.insert(selected_tag);
            }
            
            // Update filter_tags for API filtering
            self.filter_tags = self.selected_tags.clone().into_iter().collect();
            
            // Re-fetch with new tag filters
            self.clear_search_results();
            self.needs_search_models = true;
            
            self.status = format!("Tag '{}' {}", 
                if self.selected_tags.contains(&selected_tag) { "selected" } else { "deselected" },
                selected_tag
            );
        }
    } else {
        // Existing model search behavior
        self.input_mode = InputMode::Normal;
        self.popup_mode = PopupMode::None;
        self.clear_search_results();
        self.needs_search_models = true;
    }
}
```

### Available Tags Caching
Add to `src/ui/app/events.rs`:
```rust
fn update_available_tags(&mut self, models: &[ModelInfo]) {
    if self.available_tags.is_empty() {
        let mut tag_set = std::collections::HashSet<String>();
        for model in models {
            tag_set.extend(model.tags.clone());
        }
        let mut tags: Vec<String> = tag_set.into_iter().collect();
        tags.sort();
        self.available_tags = tags;
    }
}

/// Update tag cache when models change
fn refresh_tag_cache(&mut self) {
    self.available_tags.clear();
    if let Ok(models) = self.models.try_lock() {
        self.update_available_tags(&models);
    }
}
```

### API Integration
The API integration is the same as previous alternatives:

```rust
// In fetch_models_filtered()
if !tags.is_empty() {
    models.retain(|m| {
        tags.all(|tag| m.tags.contains(tag))
    });
}
```

### Enhanced Search Status
Update status display to show active tag filters:

```rust
fn get_search_status(&self) -> String {
    match self.search_mode {
        SearchMode::ModelSearch => format!("Models: '{}'", self.input.value()),
        SearchMode::TagSearch => {
            let active_count = self.selected_tags.len();
            if active_count > 0 {
                format!("Tags: {} selected - '{}'", active_count, self.input.value());
            } else {
                format!("Tags: '{}'", self.input.value());
            }
        }
    }
}
```

## User Experience

### Workflow
1. User presses `t` to enter tag search mode (or `Tab` while in existing search)
2. User types partial tag name: "text"
3. Results show: "text-generation", "text-classification", "text-summarization"
4. User navigates with `j/k` or types more to narrow down
5. User presses `Enter` to select/deselect highlighted tag
6. User can toggle between model and tag search with `Tab`
7. Selected tags persist and filter results in real-time

### Visual Feedback
- **Mode Indication:** Clear visual of which search mode is active
- **Selection State:** Selected tags shown in green/bold
- **Real-time Filtering:** Model list updates as tags are selected
- **Status Updates:** Shows "3 tags selected" in status bar
- **Navigation:** Easy toggle between model and tag search

### Integration Benefits
- **Leverages Existing:** Uses proven search popup infrastructure
- **Familiar:** Users already know how to use search
- **Flexible:** Supports both broad and specific tag selection
- **Efficient:** No additional UI space required

## Pros
- **Minimal UI:** Reuses existing search popup
- **Fast:** Text-based filtering is very fast
- **Flexible:** Supports both exact and partial matching
- **Familiar:** Users already know search interface
- **Memory Efficient:** No additional layout space needed

## Cons
- **Learning Curve:** Users need to understand dual-mode search
- **No Hierarchy:** Flat list may be overwhelming with many tags
- **Text-Dependent:** Requires knowing or guessing tag names
- **Selection Clarity:** May be unclear which tag is "currently selected"

## Search Enhancement Ideas
**Phase 1:** Basic tag search with selection
**Phase 2:** Add tag autocomplete/suggestions
**Phase 3:** Support logical operators (AND, OR, NOT)
**Phase 4:** Save/restore tag search queries

## Implementation Effort
- **Medium:** ~250-350 lines of code  
- **Risk:** Low-Medium (extends existing patterns)
- **Testing:** Medium (integration with search popup)

## File Changes Required
- `src/models.rs`: Add SearchMode enum
- `src/ui/app/state.rs`: Add tag search state
- `src/ui/app/events.rs`: Extend search popup with tag support
- `src/ui/render.rs`: Update search popup rendering
- `src/api.rs`: Ensure tag filtering works

## Migration Path
- Start with existing search popup (no new UI)
- Gradually enhance based on user feedback
- Can add dedicated tag browser later if needed
- Backward compatible with existing search behavior
