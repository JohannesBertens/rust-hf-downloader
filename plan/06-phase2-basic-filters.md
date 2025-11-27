# Phase 1: Add Tag Filtering - Alternative 2: Tag Presets System

## Overview
Implement a quick-toggle system for common tag combinations using number keys (5-9), similar to existing filter presets but specifically for tag-based filtering scenarios.

## Implementation Strategy

### State Modifications
Add to `App` struct in `src/ui/app/state.rs`:
```rust
pub tag_preset: Option<TagPreset>,           // Currently active tag preset
pub filter_tags: Vec<String>,                // Active tags (from preset + manual)
```

Add new enum to `models.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagPreset {
    NoTags,
    LLM,
    ComputerVision, 
    Audio,
    Multimodal,
    Research,
}
```

### Preset Definitions
Extend the preset system in `src/ui/app/events.rs`:

**Tag Preset Mappings:**
```rust
fn get_tag_preset_tags(&self, preset: TagPreset) -> Vec<String> {
    match preset {
        TagPreset::NoTags => vec![],
        TagPreset::LLM => vec!["text-generation", "pytorch", "transformers"],
        TagPreset::ComputerVision => vec!["image-to-text", "image-segmentation", "object-detection"],
        TagPreset::Audio => vec!["text-to-speech", "speech-recognition", "audio-classification"],
        TagPreset::Multimodal => vec!["multimodal", "vision", "text-and-visual"],
        TagPreset::Research => vec!["research", "paper", "implementation"],
    }
}
```

### Key Binding Integration
Extend the preset system in `src/ui/app/events.rs`:

**Extended Preset Keys:**
- Keep existing: `1-4` for filter presets (NoFilters, Popular, HighlyRated, Recent)
- Add: `5-9` for tag presets (NoTags, LLM, ComputerVision, Audio, Multimodal, Research)
- Add: `0` to clear all tags while keeping existing filters

**Event Handler:**
```rust
match (key.modifiers, key.code) {
    // ... existing presets 1-4 ...
    (_, KeyCode::Char('5')) => {
        if self.would_change_tags(TagPreset::NoTags) {
            self.apply_tag_preset(TagPreset::NoTags);
        }
    }
    (_, KeyCode::Char('6')) => {
        if self.would_change_tags(TagPreset::LLM) {
            self.apply_tag_preset(TagPreset::LLM);
        }
    }
    (_, KeyCode::Char('7')) => {
        if self.would_change_tags(TagPreset::ComputerVision) {
            self.apply_tag_preset(TagPreset::ComputerVision);
        }
    }
    (_, KeyCode::Char('8')) => {
        if self.would_change_tags(TagPreset::Audio) {
            self.apply_tag_preset(TagPreset::Audio);
        }
    }
    (_, KeyCode::Char('9')) => {
        if self.would_change_tags(TagPreset::Multimodal) {
            self.apply_tag_preset(TagPreset::Multimodal);
        }
    }
    (_, KeyCode::Char('0')) => {
        // Clear all tags
        if !self.filter_tags.is_empty() {
            self.clear_all_tags();
        }
    }
    _ => {}
}
```

### Tag Preset Methods
Add to `src/ui/app/events.rs`:
```rust
/// Check if applying a tag preset would change current settings
fn would_change_tags(&self, preset: TagPreset) -> bool {
    let target_tags = self.get_tag_preset_tags(preset);
    self.filter_tags != target_tags
}

/// Apply a tag preset
fn apply_tag_preset(&mut self, preset: TagPreset) {
    self.filter_tags = self.get_tag_preset_tags(preset);
    self.tag_preset = Some(preset);
    
    // Re-fetch with new tag filters
    self.clear_search_results();
    self.needs_search_models = true;
    
    self.status = format!("Tag Preset: {:?}", preset);
}

/// Clear all tags
fn clear_all_tags(&mut self) {
    self.filter_tags.clear();
    self.tag_preset = None;
    
    // Re-fetch without tag filters
    self.clear_search_results();
    self.needs_search_models = true;
    
    self.status = "All tags cleared".to_string();
}
```

### UI Rendering Updates
Update `render_filter_toolbar()` in `src/ui/render.rs`:

**Enhanced Layout:**
```
[Sort: Downloads ▼] | [Min Downloads: 10k] | [Min Likes: 100] | [Tags: LLM ▶]
```

**Tag Preset Display:**
- Show active preset name when tag preset is active
- Show individual tags when no preset (or manual selection)
- Color code presets: Green for active, Gray for available

**Visual States:**
- **No Tags:** `[Tags: None]`
- **Preset Active:** `[Tags: LLM ▶]` (with preset indicator)
- **Manual Selection:** `[Tags: text-generation, pytorch]`
- **Combined:** `[Tags: LLM + 2 custom]`

### Combined Filter & Tag System
Extend the existing filter preset logic to support combinations:

```rust
// In apply_filter_preset()
match preset {
    // ... existing FilterPreset cases ...
    FilterPreset::LLMFocused => {
        // Combines Popular + LLM tags
        self.sort_field = SortField::Downloads;
        self.sort_direction = SortDirection::Descending;
        self.filter_min_downloads = 1000;
        self.filter_min_likes = 10;
        self.apply_tag_preset(TagPreset::LLM);
        self.status = "Preset: LLM Focused (Popular LLM models)".to_string();
    }
}
```

### API Integration
Extend `fetch_models_filtered()` to support tag presets:
```rust
// Tag filtering logic remains the same
if !tags.is_empty() {
    models.retain(|m| {
        tags.all(|tag| m.tags.contains(tag))
    });
}
```

## User Experience

### Workflow
1. User applies existing filter preset (e.g., "Popular")
2. User presses `6` to apply LLM tag preset
3. Results now show "Popular LLM models" 
4. User presses `0` to clear tags, keeping "Popular" filter
5. Status shows "Filter: Popular | Tag Preset: LLM"

### Visual Feedback
- Dual display: filter preset + tag preset
- Quick toggle between preset types
- Clear indication when filters are combined
- Preset combinations shown in status: "Popular + LLM"

### Integration with Existing System
- **Orthogonal:** Tag presets work with existing filter presets
- **Independent:** Can use tag presets without filter presets
- **Combinable:** Can combine any filter preset with any tag preset
- **Override:** Manual tag editing still possible (future enhancement)

## Pros
- Very fast workflow for common use cases
- Minimal UI complexity
- Clear mental model (presets for filters, presets for tags)
- Easy to extend with more tag combinations
- No disruption to existing filter UI

## Cons
- Limited to predefined tag combinations
- No custom tag selection
- May need many preset keys for all use cases
- Tags are all-or-nothing (no partial selection)

## Preset Expansion Strategy
**Phase 1:** Core categories (LLM, ComputerVision, Audio, Multimodal)
**Phase 2:** Task-specific (Text2Img,Img2Text, Code, Math)  
**Phase 3:** Library-specific (PyTorch, TensorFlow, JAX)
**Phase 4:** Size-specific (Small, Medium, Large, Huge)

## Implementation Effort
- **Low:** ~150-200 lines of code
- **Risk:** Very Low (pure preset system, no UI restructuring)
- **Testing:** Low (add preset combinations to existing tests)

## File Changes Required
- `src/models.rs`: Add TagPreset enum
- `src/ui/app/state.rs`: Add tag preset fields
- `src/ui/app/events.rs`: Extend preset system with tag handling
- `src/ui/render.rs`: Update preset display logic
- `src/api.rs`: Ensure tag filtering works with presets

## Migration Path
- Backward compatible with existing preset system
- Users can migrate filter presets to combined filter+tag presets
- Easy to add more tag presets without breaking changes
