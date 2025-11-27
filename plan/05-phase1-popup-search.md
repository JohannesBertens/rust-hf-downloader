# Phase 1: Add Tag Filtering - Alternative 1: Simple Multi-Selection

## Overview
Extend the existing filter system to include tag-based filtering as additional filter fields, maintaining consistency with the current downloads/likes numeric filters.

## Implementation Strategy

### State Modifications
Add to `App` struct in `src/ui/app/state.rs`:
```rust
pub filter_tags: Vec<String>,           // Currently selected tags
pub available_tags: Vec<String>,        // Cached unique tags from current results
pub focused_filter_field: usize,        // Expand from 0-2 to 0-4 (add tags)
```

Add to `RenderParams` in `src/ui/render.rs`:
```rust
pub filter_tags: &'a [String],
pub available_tags: &'a [String],
pub focused_filter_field: usize,        // Updated to handle 0-4 range
```

### Filter State Management
Extend the filtering system in `src/ui/app/events.rs`:

**Key Bindings:**
- Keep existing: `f` cycles focus (now 0-4), `+/-` modifies focused field
- Add: `t` key to toggle tag selection mode
- Add: `Tab` cycles through available tags when in tag mode
- Add: `Enter` to select/deselect highlighted tag

**Tag Selection Logic:**
```rust
// In modify_focused_filter()
match self.focused_filter_field {
    0 => { /* existing sort field logic */ }
    1 => { /* existing downloads logic */ }
    2 => { /* existing likes logic */ }
    3 => { /* NEW: Tag selection mode */ 
        if delta > 0 {
            // Move to next available tag
            self.cycle_tag_selection(1);
        } else {
            // Toggle current tag selection
            self.toggle_current_tag();
        }
    }
    4 => { /* Future: Additional filter types */ }
    _ => {}
}
```

### UI Rendering Updates
Extend `render_filter_toolbar()` in `src/ui/render.rs`:

**Layout:** Expand toolbar to accommodate tag display:
```
[Sort: Downloads ▼] | [Min Downloads: 10k] | [Min Likes: 100] | [Tags: 2 selected]
```

**Visual States:**
- **Normal:** `[text-generation, pytorch]` (truncated if long)
- **Focused:** `[▴text-generation▴, pytorch]` with highlighting
- **Selected Count:** Show "2 selected" when tags active

### API Integration
Extend `fetch_models_filtered()` in `src/api.rs`:
```rust
pub async fn fetch_models_filtered(
    // ... existing params ...
    tags: &[String],        // NEW: required tags
) -> Result<Vec<ModelInfo>, reqwest::Error> {
    // ... existing logic ...
    
    // Client-side tag filtering (API doesn't support tag filters)
    if !tags.is_empty() {
        models.retain(|m| {
            tags.all(|tag| m.tags.contains(tag))
        });
    }
    
    // ... rest of existing logic ...
}
```

### Available Tags Caching
Add method to `App`:
```rust
fn update_available_tags(&mut self, models: &[ModelInfo]) {
    let mut tag_set = std::collections::HashSet<String>();
    for model in models {
        tag_set.extend(model.tags.clone());
    }
    let mut tags: Vec<String> = tag_set.into_iter().collect();
    tags.sort();
    self.available_tags = tags;
}
```

## User Experience

### Workflow
1. User searches/browses models (existing behavior)
2. User presses `f` to cycle through filter fields
3. When reaching tag field (position 3), tags display in toolbar
4. User presses `+/-` to navigate/select tags
5. Results automatically filter to show only models with selected tags
6. Status shows "Filtering by tags: text-generation, pytorch"

### Visual Feedback
- Selected tags highlighted in toolbar
- Count indicator: "Tags: 2 selected" or "Tags: All"
- Filter preset compatibility (extend existing 1-4 keys)
- Preset 5: "LLM Models" (text-generation, pytorch, transformers tags)

### Integration Benefits
- **Consistency:** Follows exact same interaction patterns as existing filters
- **Progressive:** Works alongside current filter presets
- **Performance:** Client-side filtering like existing min filters
- **Memory:** Minimal state additions, leverages existing infrastructure

## Pros
- Minimal code changes, builds on proven patterns
- Consistent UI/UX with existing filters
- Easy to understand and use
- Works with current preset system
- No layout disruptions

## Cons  
- Limited to boolean AND logic (model must have ALL selected tags)
- Toolbar may become cluttered with long tag names
- No tag hierarchy or categorization
- Tag selection is linear (no branching search)

## Implementation Effort
- **Low-Medium:** ~200-300 lines of code
- **Risk:** Low (extends proven patterns)
- **Testing:** Moderate (add tag filtering to existing test scenarios)

## File Changes Required
- `src/ui/app/state.rs`: Add tag fields
- `src/ui/app/events.rs`: Extend filter logic
- `src/ui/render.rs`: Update toolbar rendering  
- `src/api.rs`: Add tag filtering to fetch_models_filtered
- `src/config.rs`: Add tag persistence (optional)
- Update filter presets to include tag combinations
