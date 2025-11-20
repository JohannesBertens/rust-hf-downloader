# AGENT.md - AI Agent Guide for hf-search

This document provides AI agents with a comprehensive understanding of the hf-search codebase, its architecture, and how to work with it.

## Project Overview

**hf-search** is a Terminal User Interface (TUI) application written in Rust that allows users to search and browse models from the HuggingFace model hub. It provides an interactive, keyboard-driven interface with vim-like controls.

### Purpose
Enable users to discover and explore HuggingFace models directly from the terminal without needing a web browser.

### Key Technologies
- **Language**: Rust (Edition 2024)
- **UI Framework**: ratatui 0.29.0
- **Async Runtime**: tokio 1.40.0
- **HTTP Client**: reqwest 0.12 (with JSON support)
- **Terminal Backend**: crossterm 0.28.1

## Architecture

### Single-File Design
The entire application is contained in a single file: `src/main.rs` (~400 lines)

This monolithic approach simplifies:
- Understanding the complete flow
- Debugging
- Making changes to any component

### Core Components

#### 1. Data Models

```rust
// Represents a HuggingFace model from the API
struct ModelInfo {
    id: String,              // e.g., "gpt2", "bert-base-uncased"
    author: Option<String>,  // Model creator/organization
    downloads: u64,          // Total download count
    likes: u64,              // Community likes
    tags: Vec<String>,       // Classification tags
    last_modified: Option<String>,
}

// Represents a file in a model repository
struct ModelFile {
    file_type: String,       // "file" or "directory"
    path: String,            // Full path to the file
    size: u64,               // File size in bytes
}

// Represents a quantized model variant
struct QuantizationInfo {
    quant_type: String,      // e.g., "Q4_K_M", "Q5_0", "Q8_0"
    filename: String,        // Full filename
    size: u64,               // File size in bytes
}

// UI state management
enum InputMode {
    Normal,   // Browsing/navigation mode
    Editing,  // Text input mode
}

// Main application state
struct App {
    running: bool,                             // Main loop control
    event_stream: EventStream,                 // Crossterm event handler
    input: Input,                              // Text input widget
    input_mode: InputMode,                     // Current mode (Normal/Editing)
    focused_pane: FocusedPane,                 // Which list has focus
    models: Arc<Mutex<Vec<ModelInfo>>>,        // Shared model data
    list_state: ListState,                     // Model selection state
    quant_list_state: ListState,               // Quantization selection state
    loading: bool,                             // API call in progress
    error: Option<String>,                     // Last error message
    status: String,                            // Status bar text
    quantizations: Arc<Mutex<Vec<QuantizationInfo>>>, // Quantized versions
    loading_quants: bool,                      // Quantization fetch in progress
}
```

#### 2. Application Flow

```
main() 
  ├─> color_eyre::install()        // Error handler setup
  ├─> ratatui::init()              // Terminal initialization
  ├─> App::new()                   // Create app state
  ├─> App::run()                   // Main event loop
  │     ├─> terminal.draw()        // Render UI
  │     └─> handle_events()        // Process keyboard input
  └─> ratatui::restore()           // Cleanup terminal
```

#### 3. UI Layout (Four Sections)

```
┌─────────────────────────────────────┐
│ Search HuggingFace Models          │  ← Input box (3 lines)
│ [user types here]                  │
└─────────────────────────────────────┘
┌─────────────────────────────────────┐
│ Results                            │  ← Results list (flexible)
│ >> 1. gpt2 by openai ↓1.2M ♥500   │
│    2. bert-base by google ↓800K   │
│    3. ...                          │
└─────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────┐
│ Quantization Details                                        │
│      5.34 GB  Q8_0           model-name.Q8_0.gguf          │
│      4.07 GB  Q6_K           model-name.Q6_K.gguf          │
│      3.46 GB  Q5_K_M         model-name.Q5_K_M.gguf        │
│      2.87 GB  Q4_K_M         model-name.Q4_K_M.gguf        │
│      ...                                                    │
└─────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────┐
│ Status: Press '/' to search        │  ← Status bar (3 lines)
└─────────────────────────────────────┘
```

#### 4. Event Handling

**Mode-Based Input System:**

- **Normal Mode** (Yellow border on focused list)
  - `Tab`: Switch focus between Models and Quantizations
  - `j`/`↓`: Next item in focused list
  - `k`/`↑`: Previous item in focused list
  - `/`: Enter search mode
  - `Enter`: Show details for selected item
  - `q`/`Ctrl+C`: Quit

- **Editing Mode** (Yellow border on input)
  - Text input handled by tui-input widget
  - `Enter`: Execute search
  - `Esc`: Return to normal mode

**Focus System:**
- Two focusable panes: Models (default) and Quantizations
- Yellow border indicates which pane is focused
- Navigation keys (j/k/↑/↓) operate on focused pane
- Tab cycles focus between the two panes

#### 5. API Integration

**Endpoint:** `https://huggingface.co/api/models`

**Parameters:**
- `search`: URL-encoded query string
- `limit`: 50 models
- `sort`: downloads
- `direction`: -1 (descending)

**Example Request:**
```rust
async fn fetch_models(query: &str) -> Result<Vec<ModelInfo>, reqwest::Error> {
    let url = format!(
        "https://huggingface.co/api/models?search={}&limit=50&sort=downloads&direction=-1",
        urlencoding::encode(query)
    );
    let response = reqwest::get(&url).await?;
    let models: Vec<ModelInfo> = response.json().await?;
    Ok(models)
}
```

**API Response Format:**
```json
[
  {
    "modelId": "gpt2",
    "author": "openai",
    "downloads": 1234567,
    "likes": 890,
    "tags": ["text-generation", "pytorch"],
    "lastModified": "2024-01-15T10:30:00.000Z"
  }
]
```

## Code Organization

### Key Functions

| Function | Purpose |
|----------|---------|
| `main()` | Entry point, terminal setup/teardown |
| `App::new()` | Initialize application state |
| `App::run()` | Main event loop |
| `App::draw()` | Render UI components (4 panels) |
| `App::handle_crossterm_events()` | Process terminal events |
| `App::on_key_event()` | Handle keyboard input |
| `App::search_models()` | Execute API search |
| `App::toggle_focus()` | Switch focus between Models and Quantizations panes |
| `App::next_quant()` | Navigate to next quantization in list |
| `App::previous_quant()` | Navigate to previous quantization in list |
| `App::load_quantizations()` | Fetch quantized file info for selected model |
| `App::show_model_details()` | Display full model info in status bar |
| `App::show_quantization_details()` | Display quantization info in status bar |
| `fetch_models()` | HTTP request to HuggingFace models API |
| `fetch_model_files()` | HTTP request to get model file tree, handles both single files and directories |
| `is_quantization_directory()` | Check if directory name is a quantization type |
| `extract_quantization_type()` | Parse quant type from filename (dash or dot separated) |
| `format_number()` | Pretty-print large numbers (K/M suffix) |
| `format_size()` | Format bytes as KB/MB/GB |

### State Management Pattern

**Shared State with Arc<Mutex>:**
```rust
models: Arc<Mutex<Vec<ModelInfo>>>
```

Why this approach:
- Enables async API calls to update model list
- Allows UI to render while fetching data
- Thread-safe access from event handlers

### Error Handling Strategy

1. **User-Facing Errors:**
   - Stored in `App.error: Option<String>`
   - Displayed in status bar with red color
   - Cleared on next successful action

2. **Critical Errors:**
   - Bubble up via `color_eyre::Result`
   - Cause application termination with backtrace

### Quantization Details Feature

**Purpose**: Display all available quantized GGUF model variants with file sizes

**How it works:**

1. **Automatic Loading**: When user navigates models (j/k), app fetches file tree
2. **API Endpoint**: `GET https://huggingface.co/api/models/{model_id}/tree/main`
3. **File Detection**: Identifies GGUF files using two patterns:
   - **Root files**: `model.Q4_K_M.gguf` directly in repository
   - **Subdirectories**: `Q4_K_M/` folders containing split GGUF files
4. **Quantization Parsing**: Extracts quant type from filename or directory name
5. **Size Calculation**: For split files, sums all parts in the directory
6. **Sorting**: Orders by total file size (largest to smallest)
7. **Display**: Shows type, size (formatted), and representative filename

**Quantization Types Explained:**

- **Q8_0**: 8-bit quantization, highest quality, largest size (~90% original)
- **Q6_K**: 6-bit quantization, excellent quality (~70% original)
- **Q5_K_M/Q5_0**: 5-bit quantization, good quality (~60% original)
- **Q4_K_M/Q4_0**: 4-bit quantization, balanced (~50% original)
- **Q3_K_M**: 3-bit quantization, smaller size (~40% original)
- **Q2_K**: 2-bit quantization, smallest size, lower quality (~30% original)

**Suffix meanings:**
- `_K`: Uses K-quant method (improved quality)
- `_M`: Medium size/quality variant
- `_S`: Small size variant
- `_L`: Large size variant
- `_0`: Original quantization method

**Implementation Details:**

```rust
// Fetch files from model repository
async fn fetch_model_files(model_id: &str) -> Result<Vec<QuantizationInfo>> {
    let url = format!("https://huggingface.co/api/models/{}/tree/main", model_id);
    let files: Vec<ModelFile> = reqwest::get(&url).await?.json().await?;
    
    // Filter for GGUF files and extract quantization info
    let quantizations = files
        .into_iter()
        .filter(|f| f.file_type == "file" && f.path.ends_with(".gguf"))
        .filter_map(|f| {
            extract_quantization_type(&f.path).map(|quant_type| {
                QuantizationInfo {
                    quant_type,
                    filename: f.path,
                    size: f.size,
                }
            })
        })
        .collect();
    
    Ok(quantizations)
}

// Extract quantization type from various filename patterns
fn extract_quantization_type(filename: &str) -> Option<String> {
    let name = filename.trim_end_matches(".gguf");
    
    // Try dot-separated (model.Q4_K_M.gguf)
    if let Some(last) = name.split('.').last() {
        if last.starts_with('Q') {
            return Some(last.to_uppercase());
        }
    }
    
    // Try dash-separated (Qwen3-VL-30B-Q8_K_XL.gguf)
    for part in name.split('-').rev() {
        if part.starts_with('Q') {
            return Some(part.to_uppercase());
        }
    }
    
    None
}
```

**When quantizations load:**
- After search completes (for first result)
- When navigating with j/k keys
- Async fetch doesn't block UI

**Empty states:**
- "Select a model to view" - No model selected
- Empty list - Model has no GGUF files (not quantized)

**Two Organizational Patterns:**

1. **Single-file pattern** (e.g., TheBloke models):
   ```
   repo/
   ├── llama-2-7b.Q4_K_M.gguf
   ├── llama-2-7b.Q5_0.gguf
   └── llama-2-7b.Q8_0.gguf
   ```

2. **Directory pattern** (e.g., unsloth/GLM-4.6-GGUF):
   ```
   repo/
   ├── Q4_K_M/
   │   ├── GLM-4.6-Q4_K_M-00001-of-00005.gguf
   │   ├── GLM-4.6-Q4_K_M-00002-of-00005.gguf
   │   └── ... (total size summed)
   ├── Q8_0/
   │   └── GLM-4.6-Q8_0-00001-of-00003.gguf
   └── IQ4_XS/
       └── ...
   ```

The app automatically detects and handles both patterns, summing split files in directories.

## Development Guide

### Building the Project

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Check without building
cargo check

# Run directly
cargo run
```

### Adding New Features

**Example: Add sorting options**

1. Add sorting state to `App`:
```rust
struct App {
    // ... existing fields
    sort_mode: SortMode,
}

enum SortMode {
    Downloads,
    Likes,
    Recent,
}
```

2. Handle keybindings in `on_key_event()`:
```rust
KeyCode::Char('s') => {
    self.sort_mode = match self.sort_mode {
        SortMode::Downloads => SortMode::Likes,
        SortMode::Likes => SortMode::Recent,
        SortMode::Recent => SortMode::Downloads,
    };
    self.search_models().await;
}
```

3. Update API call in `fetch_models()`:
```rust
let sort_param = match sort_mode {
    SortMode::Downloads => "downloads",
    SortMode::Likes => "likes",
    SortMode::Recent => "lastModified",
};
```

### Common Modifications

#### Change Result Limit
```rust
// In fetch_models()
let url = format!(
    "https://huggingface.co/api/models?search={}&limit=100&...", // Change 50 to 100
    urlencoding::encode(query)
);
```

#### Add New Key Binding
```rust
// In App::on_key_event() under InputMode::Normal
KeyCode::Char('h') => {
    self.show_help();
}
```

#### Customize Colors
```rust
// In App::draw()
.border_style(Style::default().fg(Color::Blue))  // Change Yellow to Blue
```

## Dependencies Explanation

| Crate | Version | Purpose |
|-------|---------|---------|
| `ratatui` | 0.29.0 | TUI framework for rendering widgets |
| `crossterm` | 0.28.1 | Cross-platform terminal manipulation |
| `tokio` | 1.40.0 | Async runtime for non-blocking I/O |
| `reqwest` | 0.12 | HTTP client with JSON support |
| `serde` | 1.0 | JSON serialization/deserialization |
| `tui-input` | 0.10 | Text input widget for search box |
| `color-eyre` | 0.6.3 | Pretty error reporting |
| `futures` | 0.3.31 | Async utilities (FutureExt, StreamExt) |
| `urlencoding` | 2.1 | URL-safe query encoding |

## Testing Strategy

### Manual Testing Checklist

- [ ] Application starts without errors
- [ ] Press `/` enters search mode (yellow border)
- [ ] Type query updates input field
- [ ] Press Enter executes search
- [ ] Results populate list
- [ ] Navigate with j/k moves selection
- [ ] Press Enter shows model details
- [ ] Esc returns to normal mode
- [ ] q quits application
- [ ] Ctrl+C quits application
- [ ] Error messages display correctly
- [ ] Loading state shows during API calls

### Test Queries

```bash
# Popular models (should return results)
gpt
bert
llama
stable-diffusion

# Specific tasks
translation
image-classification
text-generation

# Edge cases
""                    # Empty query (should skip search)
xyz123notamodel       # No results
```

## Troubleshooting

### Common Issues

**Issue: Compilation errors**
- Check Rust version: `rustc --version` (needs 1.70+)
- Update dependencies: `cargo update`
- Clean build: `cargo clean && cargo build`

**Issue: UI not rendering correctly**
- Terminal too small (minimum 80x24 recommended)
- Terminal doesn't support colors (check TERM environment variable)

**Issue: API calls failing**
- Network connectivity
- HuggingFace API rate limiting
- Firewall blocking HTTPS requests

**Issue: Slow response**
- Large result sets (reduce limit in API call)
- Slow network connection
- Debug build instead of release build

## Performance Considerations

### Current Optimizations

1. **Async API Calls**: Non-blocking HTTP requests prevent UI freezing
2. **Stateful Rendering**: Only selected item state changes, not full re-render
3. **Limited Results**: 50 model cap prevents overwhelming UI
4. **Number Formatting**: K/M suffixes reduce visual clutter

### Potential Improvements

- **Pagination**: Load more results on demand
- **Caching**: Store previous search results
- **Debouncing**: Delay search until user stops typing
- **Lazy Loading**: Render only visible items in large lists

## API Limitations

### HuggingFace API Constraints

- **Rate Limiting**: Anonymous requests may be throttled
- **No Authentication**: Current implementation doesn't use API tokens
- **Search Accuracy**: Substring matching, not semantic search
- **Result Ordering**: Server-side sorting only

### Future Enhancements

To add authenticated requests:
```rust
// Add to fetch_models()
let client = reqwest::Client::new();
let response = client
    .get(&url)
    .header("Authorization", "Bearer YOUR_TOKEN")
    .send()
    .await?;
```

## Related Resources

- [Ratatui Documentation](https://docs.rs/ratatui)
- [HuggingFace API Docs](https://huggingface.co/docs/hub/api)
- [Crossterm Documentation](https://docs.rs/crossterm)
- [Tokio Documentation](https://docs.rs/tokio)

## Version History

- **v0.3.0** (Current): Added focus system with Tab key navigation
  - Tab key switches focus between Models and Quantizations lists
  - Yellow border highlights currently focused pane
  - Independent navigation (j/k) in each list
  - Enter shows details for selected item in either list
  - Separate ListState for quantization selection
  
- **v0.2.0**: Added quantization details panel showing GGUF file sizes and types
  - New 4-panel UI layout with dedicated quantization section
  - Automatic loading of quantization info when navigating models
  - File size formatting (GB/MB/KB)
  - Quantization type parsing with size-based sorting (largest first)
  - Support for K-quant variants and directory-based organization
  
- **v0.1.0**: Initial implementation with basic search and navigation

## Contributing

When modifying this codebase:

1. Maintain the single-file structure unless complexity demands modularization
2. Keep async/await pattern for API calls
3. Preserve vim-like keybindings for consistency
4. Update this AGENT.md file with architectural changes
5. Test all input modes and edge cases
6. Ensure error messages are user-friendly

---

**Last Updated**: 2025-11-20  
**Maintainer**: Johannes Bertens <>
