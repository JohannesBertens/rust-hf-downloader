# Phase 1: Add Tag Filtering - Alternative 3: Hierarchical Tag Browser

## Overview
Implement a tree-like navigation system for tags with hierarchical categorization, allowing users to browse tags like exploring folders in a file system.

## Implementation Strategy

### State Modifications
Add to `App` struct in `src/ui/app/state.rs`:
```rust
pub tag_tree: Option<TagTreeNode>,          // Hierarchical tag structure
pub tag_browser_focused_pane: TagBrowserPane, // Which tag browser pane is focused
pub tag_selection: HashSet<String>,         // Currently selected tags
pub tag_browser_state: ListState,           // Navigation state in tag browser
```

Add new types to `models.rs`:
```rust
#[derive(Debug, Clone)]
pub struct TagTreeNode {
    pub name: String,
    pub tags: Vec<String>,                   // Direct tags in this category
    pub children: Vec<TagTreeNode>,          // Subcategories
    pub expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagBrowserPane {
    Categories,    // Navigate categories
    Tags,         // Navigate tags in category  
    Active,       // View active selections
}
```

### Tag Tree Structure
Build hierarchical tag categories in `src/api.rs` or new `src/tag_hierarchy.rs`:

```rust
pub fn build_tag_hierarchy(models: &[ModelInfo]) -> TagTreeNode {
    let mut root = TagTreeNode {
        name: "All Tags".to_string(),
        tags: vec![],
        children: vec![],
        expanded: true,
    };
    
    // Analyze all tags to build categories
    let mut categories: HashMap<String, Vec<String>> = HashMap::new();
    
    for model in models {
        for tag in &model.tags {
            let (category, tag_name) = categorize_tag(tag);
            categories
                .entry(category)
                .or_default()
                .push(tag_name);
        }
    }
    
    // Build tree structure
    for (category, tags) in categories {
        let mut category_node = TagTreeNode {
            name: category,
            tags: vec![],
            children: vec![],
            expanded: false,
        };
        
        // Further categorize if needed
        if tags.len() > 10 {
            // Split into subcategories
            category_node.children = build_subcategories(&tags);
        } else {
            category_node.tags = tags;
        }
        
        root.children.push(category_node);
    }
    
    root
}

fn categorize_tag(tag: &str) -> (String, String) {
    // AI/ML Categories
    if tag.starts_with("text-") || tag.contains("llm") || tag.contains("language") {
        ("Language Models".to_string(), tag.to_string())
    }
    // Computer Vision  
    else if tag.contains("image") || tag.contains("vision") || tag.contains("visual") {
        ("Computer Vision".to_string(), tag.to_string())
    }
    // Audio/Speech
    else if tag.contains("audio") || tag.contains("speech") || tag.contains("speech") {
        ("Audio Processing".to_string(), tag.to_string())
    }
    // Multimodal
    else if tag.contains("multimodal") || tag.contains("vision") {
        ("Multimodal".to_string(), tag.to_string())
    }
    // Libraries
    else if tag.contains("pytorch") || tag.contains("tensorflow") {
        ("Libraries".to_string(), tag.to_string())
    }
    // Tasks
    else if tag.contains("generation") || tag.contains("classification") {
        ("Tasks".to_string(), tag.to_string())
    }
    // Research
    else if tag.contains("research") || tag.contains("paper") {
        ("Research".to_string(), tag.to_string())
    }
    // Default
    else {
        ("Other".to_string(), tag.to_string())
    }
}
```

### UI Layout Restructuring
Create dedicated tag browser area in `src/ui/render.rs`:

**New Layout (4 panes instead of 3):**
```
┌─────────┬─────────┬───────────────┬───────────────┐
│Filters  │Models   │Tag Browser   │Model Details │
│Toolbar  │List     │(NEW)         │(If needed)   │
│         │         │               │               │
│         │         │               │               │
└─────────┴─────────┴───────────────┴───────────────┘
```

**Tag Browser Layout:**
```
┌─Tag Browser────┐
│Categories    ▼ │ <- Focused pane
├─Language      ▼ │
│  ├─text-gen  ✓ │ <- Selected tag
│  ├─chat      ○ │
│  └─summarize ○ │
├─Computer Vis ▼ │
│  ├─image-gen ○ │
│  └─segment   ○ │
└─Active (2)────┘
```

### Event Handling Integration
Add to `src/ui/app/events.rs`:

**New Key Bindings:**
- `g` - Open/close tag browser (toggle new pane focus)
- `Tab` - Cycle through panes: Models → TagBrowser → ModelDetails → Models
- `Enter` in Categories - Expand/collapse category
- `Enter` in Tags - Toggle tag selection
- `Space` - Quick tag toggle (alternative to Enter)
- `c` - Clear all selected tags

**Integration with existing navigation:**
```rust
// Extend toggle_focus() to include tag browser
fn toggle_focus(&mut self) {
    self.focused_pane = match self.focused_pane {
        FocusedPane::Models => FocusedPane::TagBrowser,
        FocusedPane::TagBrowser => {
            if self.display_mode == ModelDisplayMode::Standard {
                FocusedPane::ModelMetadata
            } else {
                FocusedPane::QuantizationGroups
            }
        }
        FocusedPane::ModelMetadata | FocusedPane::QuantizationGroups => FocusedPane::Models,
        FocusedPane::FileTree | FocusedPane::QuantizationFiles => FocusedPane::Models,
    };
}
```

**Tag Browser Event Handling:**
```rust
// Add new match case in handle_normal_mode_input
(_, KeyCode::Char('g')) => {
    if self.focused_pane == FocusedPane::TagBrowser {
        self.toggle_focus(); // Exit tag browser
    } else {
        // Enter tag browser
        if let Ok(models) = self.models.try_lock() {
            self.tag_tree = Some(build_tag_hierarchy(&models));
        }
        self.focused_pane = FocusedPane::TagBrowser;
        self.status = "Tag Browser - Use Tab to navigate, Enter to select".to_string();
    }
}
```

### Tag Browser Rendering
Create `render_tag_browser()` in `src/ui/render.rs`:

```rust
pub fn render_tag_browser(
    frame: &mut Frame,
    area: Rect,
    tag_tree: &Option<TagTreeNode>,
    focused_pane: TagBrowserPane,
    browser_state: &mut ListState,
    selection: &HashSet<String>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Tag Browser  [g: Toggle | Tab: Navigate | Enter: Select | c: Clear]")
        .style(Style::default().fg(Color::Green));
    
    let inner = block.inner(area);
    frame.render_widget(block, area);
    
    if let Some(tree) = tag_tree {
        render_tag_tree_node(
            frame,
            inner,
            tree,
            0,
            focused_pane,
            browser_state,
            selection,
        );
    } else {
        let paragraph = Paragraph::new("Loading tag hierarchy...".to_string());
        frame.render_widget(paragraph, inner);
    }
}
```

### Tag Selection Logic
Add to `src/ui/app/events.rs`:

```rust
fn toggle_tag_selection(&mut self, tag: &str) {
    if self.tag_selection.contains(tag) {
        self.tag_selection.remove(tag);
    } else {
        self.tag_selection.insert(tag.to_string());
    }
    
    // Update filter_tags for API filtering
    self.filter_tags = self.tag_selection.clone().into_iter().collect();
    
    // Re-fetch with new tag filters
    self.clear_search_results();
    self.needs_search_models = true;
    
    self.status = format!("Tag selection: {} tags active", self.filter_tags.len());
}

fn clear_all_tags(&mut self) {
    self.tag_selection.clear();
    self.filter_tags.clear();
    
    self.clear_search_results();
    self.needs_search_models = true;
    
    self.status = "All tag selections cleared".to_string();
}
```

### Integration with API
The `filter_tags` field already exists in Alternative 1, so the API integration is the same:

```rust
// In fetch_models_filtered()
if !tags.is_empty() {
    models.retain(|m| {
        tags.all(|tag| m.tags.contains(tag))
    });
}
```

### Performance Considerations
**Lazy Loading:** Build tag hierarchy only when tag browser is opened
**Caching:** Cache tag hierarchy until search results change significantly  
**Memory:** Use HashSet for O(1) tag lookup in selection

## User Experience

### Workflow
1. User presses `g` to open tag browser pane
2. User navigates categories with `j/k` (down/up)
3. User expands category with `Enter`
4. User selects specific tags with `Enter` (checkmarks appear)
5. Results update in real-time as tags are selected
6. User presses `g` again or `Tab` to return to model list

### Visual Feedback
- **Categories:** Triangle indicators (▼ expanded, ▶ collapsed)
- **Selected Tags:** Checkmarks (✓) in tag list
- **Active Count:** "Active (3)" showing current selection
- **Real-time:** Model list updates immediately on tag selection
- **Focus:** Clear indication of which pane has keyboard focus

## Pros
- **Intuitive:** Feels like exploring a file system
- **Scalable:** Handles large number of tags naturally
- **Flexible:** Supports both broad categories and specific tags
- **Visual:** Clear hierarchy and selection states
- **Powerful:** Complex filtering without complex UI

## Cons
- **Complex:** More moving parts than simple approaches
- **Space:** Takes up significant screen real estate
- **Learning:** Users need to learn new navigation patterns
- **Performance:** More computation for hierarchy building

## Implementation Effort
- **High:** ~400-500 lines of code
- **Risk:** Medium (significant UI restructuring)
- **Testing:** High (hierarchy, selection, navigation, integration)

## File Changes Required
- `src/models.rs`: Add TagTreeNode and related types
- `src/ui/app/state.rs`: Add tag browser state
- `src/ui/app/events.rs`: Add tag browser navigation
- `src/ui/render.rs`: Add tag browser rendering
- `src/api.rs` or new `src/tag_hierarchy.rs`: Tag categorization logic
- Layout changes to support 4th pane

## Extension Possibilities
- **Search:** Add tag search within browser
- **Custom:** Allow users to create custom categories
- **History:** Remember frequently used tag combinations
- **Export:** Share tag filter configurations
