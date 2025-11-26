# Phase 1: Popup Search (MVP)

## Goal
Move the inline search bar to a popup dialog, freeing up the top bar for filters while maintaining all existing search functionality.

## Changes Required

### 1. Add SearchPopup to PopupMode Enum

**File**: `src/models.rs`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupMode {
    None,
    DownloadPath,
    ResumeDownload,
    Options,
    AuthError { model_url: String },
    SearchPopup,  // NEW
}
```

### 2. Trigger Search Popup

**File**: `src/ui/app/events.rs`

**In `handle_normal_mode_input()` method**, modify the `/` key handler:

```rust
(_, KeyCode::Char('/')) => {
    // NEW: Open search popup instead of entering editing mode
    self.popup_mode = PopupMode::SearchPopup;
    self.input.reset(); // Clear previous search
    self.input_mode = InputMode::Editing;
    self.status = "Enter search query, press Enter to search, ESC to cancel".to_string();
}
```

### 3. Add Search Popup Event Handler

**File**: `src/ui/app/events.rs`

**Add new method** to `impl App`:

```rust
/// Handle keyboard input in Search popup
async fn handle_search_popup_input(&mut self, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            self.input_mode = InputMode::Normal;
            self.popup_mode = PopupMode::None;
            // Clear results immediately before searching
            self.clear_search_results();
            // Set flag to search on next iteration (allows UI to render first)
            self.needs_search_models = true;
        }
        KeyCode::Esc => {
            self.input_mode = InputMode::Normal;
            self.popup_mode = PopupMode::None;
            self.status = "Press '/' to search, Tab to switch panes, 'd' to download, 'v' to verify, 'o' for options, 'q' to quit".to_string();
        }
        _ => {
            self.input.handle_event(&Event::Key(key));
        }
    }
}
```

**In `on_key_event()` method**, add popup dispatch before existing popup checks:

```rust
pub async fn on_key_event(&mut self, key: KeyEvent) {
    self.error = None;

    // Handle popup input separately
    if self.popup_mode == PopupMode::SearchPopup {
        self.handle_search_popup_input(key).await;
        return;
    } else if self.popup_mode == PopupMode::Options {
        // ... existing code ...
```

### 4. Remove Inline Search Mode

**File**: `src/ui/app/events.rs`

**In `handle_editing_mode_input()` method**, this becomes obsolete for search but keep it for now as it may be used elsewhere. It will no longer be called for search operations.

### 5. Update Main UI Layout

**File**: `src/ui/render.rs`

**In `render_ui()` function**, update the layout:

```rust
pub fn render_ui(frame: &mut Frame, params: RenderParams) {
    // ... existing params destructuring ...
    
    // NEW LAYOUT: Remove search input, add placeholder for future filter toolbar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Placeholder for filter toolbar (temporary empty)
            Constraint::Min(10),     // Results list (gets more space!)
            Constraint::Length(12),  // Bottom panel
            Constraint::Length(4),   // Status
        ])
        .split(frame.area());

    // TEMPORARY: Render empty placeholder block for filter toolbar
    let placeholder_block = Block::default()
        .borders(Borders::ALL)
        .title("Filters (Coming in Phase 2) - Press '/' to search");
    frame.render_widget(placeholder_block, chunks[0]);

    // Results list (was chunks[1], still chunks[1])
    let items: Vec<ListItem> = models
        .iter()
        // ... existing list rendering code ...
```

**Remove old search input rendering** (the first widget in current render_ui):
```rust
// DELETE THIS ENTIRE SECTION:
// Search input box
let input_block = Block::default()
    .borders(Borders::ALL)
    .title("Search HuggingFace Models")
    // ... entire search input widget ...
```

### 6. Add Search Popup Rendering

**File**: `src/ui/render.rs`

**Add new function** at the end of the file:

```rust
/// Render search popup dialog
pub fn render_search_popup(
    frame: &mut Frame,
    input: &Input,
) {
    // Calculate centered popup area
    let popup_width = 60.min(frame.area().width.saturating_sub(4));
    let popup_height = 9;
    let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
    let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
    
    let popup_area = Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };
    
    // Clear the popup area first to remove any underlying content
    frame.render_widget(Clear, popup_area);
    
    // Render popup background
    let popup_block = Block::default()
        .borders(Borders::ALL)
        .title("Search HuggingFace Models")
        .style(Style::default().fg(Color::Yellow).bg(Color::Black));
    
    frame.render_widget(popup_block, popup_area);
    
    // Render input label
    let label_area = Rect {
        x: popup_area.x + 2,
        y: popup_area.y + 1,
        width: popup_area.width.saturating_sub(4),
        height: 1,
    };
    
    let label = Paragraph::new("Query:")
        .style(Style::default().fg(Color::White));
    
    frame.render_widget(label, label_area);
    
    // Render input field
    let input_area = Rect {
        x: popup_area.x + 2,
        y: popup_area.y + 2,
        width: popup_area.width.saturating_sub(4),
        height: 1,
    };
    
    let width = input_area.width.max(3) as usize;
    let scroll = input.visual_scroll(width);
    
    let input_widget = Paragraph::new(input.value())
        .style(Style::default().fg(Color::Yellow))
        .scroll((0, scroll as u16));
    
    frame.render_widget(input_widget, input_area);
    
    // Set cursor position
    frame.set_cursor_position((
        input_area.x + ((input.visual_cursor()).max(scroll) - scroll) as u16,
        input_area.y,
    ));
    
    // Render instructions
    let instructions_area = Rect {
        x: popup_area.x + 2,
        y: popup_area.y + 4,
        width: popup_area.width.saturating_sub(4),
        height: 1,
    };
    
    let instructions = Paragraph::new("Press Enter to search, ESC to cancel")
        .style(Style::default().fg(Color::DarkGray));
    
    frame.render_widget(instructions, instructions_area);
}
```

### 7. Call Search Popup Renderer

**File**: `src/main.rs` (or wherever render loop is)

**In the main render loop**, after `render_ui()` is called, add conditional popup rendering:

```rust
// After render_ui() call, check for search popup
if app.popup_mode == PopupMode::SearchPopup {
    ui::render::render_search_popup(&mut frame, &app.input);
}

// Existing popup renderers (resume, download path, options, etc.)
if app.popup_mode == PopupMode::ResumeDownload && !app.incomplete_downloads.is_empty() {
    ui::render::render_resume_popup(&mut frame, &app.incomplete_downloads);
}
// ... other popups ...
```

## Testing Checklist

- [ ] Press `/` key opens search popup
- [ ] Popup is centered and styled correctly
- [ ] Can type search query in popup
- [ ] Cursor position is correct
- [ ] Enter key executes search and closes popup
- [ ] ESC key closes popup without searching
- [ ] Search results display correctly
- [ ] Status bar shows appropriate messages
- [ ] No cursor visible after popup closes
- [ ] Top bar now has empty placeholder

## Visual Before/After

### Before (Current)
```
┌─ Search HuggingFace Models ────────────┐
│ mistral                                 │
└─────────────────────────────────────────┘
┌─ Results ──────────────────────────────┐
│ >> 1. mistralai/Mistral-7B-v0.1        │
│    2. mistralai/Mixtral-8x7B-v0.1      │
│    ...                                  │
```

### After (Phase 1)
```
┌─ Filters (Coming in Phase 2) - Press '/' to search ─┐
│                                                       │
└───────────────────────────────────────────────────────┘
┌─ Results ─────────────────────────────────────────────┐
│ >> 1. mistralai/Mistral-7B-v0.1                      │
│    2. mistralai/Mixtral-8x7B-v0.1                     │
│    ...                                                 │

[When '/' is pressed, popup appears:]

    ┌─ Search HuggingFace Models ───────────┐
    │ Query:                                 │
    │ mistral█                               │
    │                                        │
    │ Press Enter to search, ESC to cancel  │
    └────────────────────────────────────────┘
```

## Notes

- No state changes required in `src/ui/app/state.rs` (reusing existing `input` field)
- No API changes required (using existing search endpoint)
- Search behavior is identical, only the UI mechanism changes
- This phase is fully functional on its own and can be released independently
